//! The decision log: what is written, what is read back, and what is refused.

use std::fs;

use crate::support::*;
use lighthouse_model::Judgment;
use lighthouse_store::{Error, Observed, Standing, Store};
use tempfile::TempDir;

/// A project root that has recorded `findings` and judged some of them.
fn clone_with(findings: &[Observed], judged: &[(&str, Judgment)]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.record(&run(findings.to_vec())).unwrap();
    for (fingerprint, judgment) in judged {
        judge(&mut store, fingerprint, *judgment);
    }
    dir
}

#[test]
fn a_decision_log_carries_judgments_and_suppressions_to_another_clone() {
    let findings = [
        observed("f1", "design/a", "a.go"),
        observed("f2", "design/a", "b.go"),
    ];
    let author = tempfile::tempdir().unwrap();
    let mut store = Store::open(author.path()).unwrap();
    store.record(&run(findings.to_vec())).unwrap();
    judge_suppressed(&mut store, "f1", "named policy");
    drop(store);
    assert_eq!(
        log_of(author.path()).lines().count(),
        2,
        "a judgment and its suppression"
    );

    let teammate = tempfile::tempdir().unwrap();
    fs::create_dir_all(teammate.path().join(".lighthouse")).unwrap();
    fs::write(Store::log_path_in(teammate.path()), log_of(author.path())).unwrap();
    let mut store = Store::open_existing(teammate.path()).unwrap().unwrap();
    store.record(&run(findings.to_vec())).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));
    assert_eq!(standing_of(&store, "f2"), None);
    let history = store.history("f1").unwrap();
    assert_eq!(history[0].judgment, Judgment::Fail);
    assert_eq!(history[0].was_attributed_to.id.as_deref(), Some("claude"));
    assert_eq!(history[0].suppressions[0].justification, "named policy");
}

#[test]
fn a_judgment_is_logged_before_the_cache_and_the_cache_is_rebuilt_from_the_log() {
    let findings = [observed("f1", "design/a", "a.go")];
    let dir = clone_with(&findings, &[("f1", Judgment::Pass)]);
    drop(Store::open(dir.path()).unwrap());
    fs::remove_file(Store::path_in(dir.path())).unwrap();
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", Store::path_in(dir.path()).display()));
    }
    let mut rebuilt = Store::open(dir.path()).unwrap();
    assert_eq!(rebuilt.history("f1").unwrap().len(), 1);
    rebuilt.record(&run(findings.to_vec())).unwrap();
    assert_eq!(standing_of(&rebuilt, "f1"), Some(Standing::Suppressed));
}

#[test]
fn a_log_that_is_missing_is_written_again_from_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    judge_suppressed(&mut store, "f1", "named policy");
    drop(store);
    let log = log_of(dir.path());
    fs::remove_file(Store::log_path_in(dir.path())).unwrap();

    drop(Store::open(dir.path()).unwrap());

    assert_eq!(log.lines().count(), 2);
    let written = log_of(dir.path());
    let mut again: Vec<&str> = written.lines().collect();
    let mut was: Vec<&str> = log.lines().collect();
    again.sort_unstable();
    was.sort_unstable();
    assert_eq!(again, was, "the same two records");
}

#[test]
fn concatenated_logs_apply_every_decision_once() {
    let findings = [
        observed("f1", "design/a", "a.go"),
        observed("f2", "design/a", "b.go"),
    ];
    let one = clone_with(&findings, &[("f1", Judgment::Pass)]);
    let two = clone_with(&findings, &[("f2", Judgment::NotApplicable)]);
    let merged = tempfile::tempdir().unwrap();
    fs::create_dir_all(merged.path().join(".lighthouse")).unwrap();
    let union = format!(
        "{}{}{}",
        log_of(one.path()),
        log_of(two.path()),
        log_of(one.path())
    );
    fs::write(Store::log_path_in(merged.path()), union).unwrap();

    let mut store = Store::open(merged.path()).unwrap();
    store.record(&run(findings.to_vec())).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));
    assert_eq!(standing_of(&store, "f2"), Some(Standing::Suppressed));
    assert_eq!(
        store.history("f1").unwrap().len(),
        1,
        "the duplicate line is one record"
    );
    assert_eq!(store.history("f2").unwrap().len(), 1);
    drop(store);
    assert_eq!(
        log_of(merged.path()).lines().count(),
        3,
        "the log is left as merged"
    );
}

#[test]
fn log_lines_are_canonical_and_unchanged_by_reading() {
    let dir = clone_with(
        &[observed("f1", "design/a", "a.go")],
        &[("f1", Judgment::Pass)],
    );
    let log = log_of(dir.path());
    let line = log.lines().next().unwrap();
    let value: serde_json::Value = serde_json::from_str(line).unwrap();
    let keys: Vec<&String> = value["spec"].as_object().unwrap().keys().collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "keys are sorted");
    assert!(
        line.starts_with(
            "{\"apiVersion\":\"lighthouse/v1alpha1\",\"kind\":\"Judgment\",\"metadata\":{\"name\":"
        ),
        "{line}"
    );
    assert_eq!(value["spec"]["decisionName"], "design/a");
    assert_eq!(value["spec"]["judgment"], "pass");
    assert_eq!(value["spec"]["meaningVersion"], "sem1");
    assert_eq!(value["spec"]["wasAttributedTo"]["type"], "SoftwareAgent");
    assert!(value["spec"]["generatedAtTime"].is_string());
    assert!(
        !line.contains(": ") && !line.contains(", "),
        "compact: {line}"
    );
    drop(Store::open(dir.path()).unwrap());
    assert_eq!(log_of(dir.path()), log);
}

#[test]
fn an_unreadable_or_altered_log_line_is_an_error_naming_the_line() {
    let dir = clone_with(
        &[observed("f1", "design/a", "a.go")],
        &[("f1", Judgment::Pass)],
    );
    let log = log_of(dir.path());
    let path = Store::log_path_in(dir.path());

    fs::write(&path, format!("{log}not json\n")).unwrap();
    let error = Store::open(dir.path()).err().unwrap();
    assert!(matches!(error, Error::Log { line: 2, .. }), "{error}");

    fs::write(
        &path,
        log.replace("\"judgment\":\"pass\"", "\"judgment\":\"fail\""),
    )
    .unwrap();
    let error = Store::open(dir.path()).err().unwrap();
    assert!(
        error.to_string().contains("the id does not match"),
        "{error}"
    );
}

#[test]
fn a_line_of_any_other_kind_is_an_error_naming_the_file_and_the_line() {
    let dir = clone_with(
        &[observed("f1", "design/a", "a.go")],
        &[("f1", Judgment::Pass)],
    );
    let log = log_of(dir.path());
    let path = Store::log_path_in(dir.path());
    let verdict = r#"{"apiVersion":"lighthouse/v1alpha1","kind":"Verdict","metadata":{"name":"v1"},"spec":{"fingerprint":"f9"}}"#;
    let flat = r#"{"fingerprint":"f9","id":"x","verdict":"rejected"}"#;
    let rewrite = r#"{"apiVersion":"lighthouse/v1alpha1","kind":"Rewrite","metadata":{"name":"r1"},"spec":{}}"#;
    let newer = log.replace("v1alpha1", "v9");

    for (line, mention) in [
        (verdict, "`Verdict` is not a kind of the decision log"),
        (flat, "no `kind`"),
        (rewrite, "`Rewrite` is not a kind of the decision log"),
        (newer.as_str().trim_end(), "`apiVersion`"),
    ] {
        fs::write(&path, format!("{log}{line}\n")).unwrap();
        let error = Store::open(dir.path()).err().unwrap();
        let message = error.to_string();
        assert!(matches!(error, Error::Log { line: 2, .. }), "{message}");
        assert!(message.contains("decisions.jsonl"), "{message}");
        assert!(message.contains(mention), "{message}");
    }
}

#[test]
fn an_append_after_a_line_without_a_newline_starts_a_new_line() {
    let dir = clone_with(
        &[observed("f1", "design/a", "a.go")],
        &[("f1", Judgment::Pass)],
    );
    let path = Store::log_path_in(dir.path());
    let log = log_of(dir.path());
    fs::write(&path, log.trim_end()).unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    judge(&mut store, "f1", Judgment::Fail);
    assert_eq!(log_of(dir.path()).lines().count(), 2);
    drop(store);
    assert_eq!(
        Store::open(dir.path())
            .unwrap()
            .history("f1")
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn a_judgment_with_a_field_this_build_does_not_know_is_still_read() {
    let dir = clone_with(
        &[observed("f1", "design/a", "a.go")],
        &[("f1", Judgment::Pass)],
    );
    let path = Store::log_path_in(dir.path());
    let mut record: serde_json::Value = serde_json::from_str(log_of(dir.path()).trim()).unwrap();
    record["spec"]["futureField"] = serde_json::json!({ "from": "a newer build" });
    let canonical = serde_json::to_string(&record["spec"]).unwrap();
    record["metadata"]["name"] = lighthouse_model::hash::short(&canonical, 16).into();
    fs::write(&path, format!("{record}\n")).unwrap();
    fs::remove_file(Store::path_in(dir.path())).unwrap();

    let store = Store::open(dir.path()).unwrap();

    assert_eq!(store.history("f1").unwrap().len(), 1);
}

#[test]
fn to_json_shows_a_judgment_in_camel_case_with_its_id_label_and_suppressions() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    judge_suppressed(&mut store, "f1", "named policy");
    let event = store.history("f1").unwrap().remove(0);

    let json = event.to_json();

    assert_eq!(json["decisionName"], "design/a");
    assert_eq!(json["judgment"], "fail");
    assert_eq!(json["wasAttributedTo"]["id"], "claude");
    assert_eq!(json["id"], event.id.as_str());
    assert_eq!(json["label"], "separate");
    assert_eq!(json["suppressions"][0]["kind"], "external");
    assert_eq!(json["suppressions"][0]["status"], "accepted");
    assert_eq!(json["suppressions"][0]["justification"], "named policy");
    assert!(json.get("decision_name").is_none());
}

#[test]
fn the_judgment_and_suppression_records_have_schemas() {
    let kinds: Vec<_> = lighthouse_store::descriptors()
        .iter()
        .map(|d| d.kind)
        .collect();
    assert_eq!(kinds, ["Judgment", "Suppression"]);
}

#[test]
fn a_judgment_spec_is_what_a_recorded_judgment_shows() {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    let event = store
        .resolve(&review("f1", Judgment::Pass), stamp("sem1"))
        .unwrap()
        .event;

    let spec: lighthouse_store::JudgmentSpec = serde_json::from_value(event.to_json()).unwrap();

    assert_eq!(spec.decision_name, "design/a");
    assert_eq!(spec.judgment, Judgment::Pass);
    assert_eq!(spec.meaning_version.as_deref(), Some("sem1"));
    assert_eq!(spec.generated_at_time, event.generated_at_time);
}
