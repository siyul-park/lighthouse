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
fn a_deleted_log_is_not_written_again_from_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    judge_suppressed(&mut store, "f1", "named policy");
    drop(store);
    fs::remove_file(Store::log_path_in(dir.path())).unwrap();

    let store = Store::open(dir.path()).unwrap();

    assert!(
        store.history("f1").unwrap().is_empty(),
        "the log is the truth"
    );
    assert_eq!(standing_of(&store, "f1"), None);
    drop(store);
    assert!(
        !Store::log_path_in(dir.path()).exists(),
        "and opening wrote nothing"
    );
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
    let signal = r#"{"apiVersion":"lighthouse/v1alpha1","kind":"Signal","metadata":{"name":"s1"},"spec":{}}"#;
    let flat = r#"{"fingerprint":"f9","id":"x"}"#;
    let newer = log.replace("v1alpha1", "v9");

    for (line, mention) in [
        (signal, "`Signal` is not a kind of the decision log"),
        (flat, "no `kind`"),
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

#[test]
fn a_branch_switch_shows_only_the_judgments_of_the_log_that_is_there() {
    let findings = [
        observed("f1", "design/a", "a.go"),
        observed("f2", "design/a", "b.go"),
    ];
    let main = clone_with(&findings, &[("f1", Judgment::Pass)]);
    let feature = clone_with(&findings, &[("f2", Judgment::NotApplicable)]);

    // One working tree, one cache, the log of one branch and then of the other.
    let tree = tempfile::tempdir().unwrap();
    fs::create_dir_all(tree.path().join(".lighthouse")).unwrap();
    let log = Store::log_path_in(tree.path());
    fs::write(&log, log_of(main.path())).unwrap();
    {
        let mut store = Store::open(tree.path()).unwrap();
        store.record(&run(findings.to_vec())).unwrap();
        assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));
        assert_eq!(standing_of(&store, "f2"), None);
    }

    fs::write(&log, log_of(feature.path())).unwrap();
    let store = Store::open(tree.path()).unwrap();
    assert_eq!(standing_of(&store, "f1"), None, "main's judgment is gone");
    assert_eq!(standing_of(&store, "f2"), Some(Standing::Suppressed));
    assert!(store.history("f1").unwrap().is_empty());
    assert_eq!(
        log_of(tree.path()),
        log_of(feature.path()),
        "opening wrote nothing to the log"
    );
}

#[test]
fn a_removed_log_line_stays_removed() {
    let dir = clone_with(
        &[observed("f1", "design/a", "a.go")],
        &[("f1", Judgment::Pass), ("f1", Judgment::Fail)],
    );
    let log = log_of(dir.path());
    assert_eq!(log.lines().count(), 2);
    let kept: String = log.lines().take(1).map(|l| format!("{l}\n")).collect();
    fs::write(Store::log_path_in(dir.path()), &kept).unwrap();

    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.history("f1").unwrap().len(), 1);
    drop(store);
    assert_eq!(log_of(dir.path()), kept, "and the log was not written back");
    assert_eq!(
        Store::open(dir.path())
            .unwrap()
            .history("f1")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn concurrent_opens_and_resolves_write_each_line_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut seed = Store::open(dir.path()).unwrap();
    seed.record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    drop(seed);
    let root = dir.path().to_owned();
    let workers: Vec<_> = (0..4)
        .map(|n| {
            let root = root.clone();
            std::thread::spawn(move || {
                for round in 0..5 {
                    let mut store = Store::open(&root).unwrap();
                    if round % 2 == 0 {
                        let mut input = review("f1", Judgment::Fail);
                        input.reason = Some(format!("worker {n} round {round}"));
                        store.resolve(&input, stamp("sem1")).unwrap();
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }

    let log = log_of(dir.path());
    let lines: Vec<&str> = log.lines().collect();
    let distinct: std::collections::BTreeSet<&str> = lines.iter().copied().collect();
    assert_eq!(lines.len(), 12, "four workers, three resolves each");
    assert_eq!(distinct.len(), lines.len(), "no duplicate line");
    assert_eq!(
        Store::open(dir.path())
            .unwrap()
            .history("f1")
            .unwrap()
            .len(),
        12
    );
}

#[test]
fn a_judgment_and_its_suppression_are_one_write_to_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    judge_suppressed(&mut store, "f1", "named policy");
    let log = log_of(dir.path());
    let kinds: Vec<String> = log
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(kinds, ["Judgment", "Suppression"]);
    assert!(log.ends_with('\n'));
}

#[test]
fn a_last_line_an_interrupted_write_cut_short_is_skipped_with_a_notice() {
    let dir = clone_with(
        &[observed("f1", "design/a", "a.go")],
        &[("f1", Judgment::Pass)],
    );
    let path = Store::log_path_in(dir.path());
    let log = log_of(dir.path());
    fs::write(&path, format!("{log}{{\"apiVersion\":\"lighthouse/v1al")).unwrap();

    let store = Store::open(dir.path()).unwrap();

    assert_eq!(store.history("f1").unwrap().len(), 1);
    assert_eq!(store.notices().len(), 1, "{:?}", store.notices());
    assert!(store.notices()[0].contains("line 2 is cut short"));
    drop(store);

    // The next judgment cuts the torn line off instead of gluing to it.
    let mut store = Store::open(dir.path()).unwrap();
    judge(&mut store, "f1", Judgment::Fail);
    drop(store);
    let healed = Store::open(dir.path()).unwrap();
    assert!(healed.notices().is_empty());
    assert_eq!(healed.history("f1").unwrap().len(), 2);

    // A bad line anywhere else is still an error.
    fs::write(&path, format!("garbage\n{log}")).unwrap();
    let error = Store::open(dir.path()).err().unwrap();
    assert!(matches!(error, Error::Log { line: 1, .. }), "{error}");
}

#[test]
fn a_suppression_without_its_judgment_is_ignored_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![
            observed("f1", "design/a", "a.go"),
            observed("f2", "design/a", "b.go"),
        ]))
        .unwrap();
    judge_suppressed(&mut store, "f1", "named policy");
    judge(&mut store, "f2", Judgment::Fail);
    drop(store);
    let log = log_of(dir.path());
    let mut lines: Vec<&str> = log.lines().collect();
    let suppression = lines.remove(1).to_owned();
    let path = Store::log_path_in(dir.path());

    // Its judgment is missing.
    fs::write(&path, format!("{}\n{suppression}\n", lines[1])).unwrap();
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.notices().len(), 1, "{:?}", store.notices());
    assert_eq!(standing_of(&store, "f1"), None);
    assert_eq!(standing_of(&store, "f2"), Some(Standing::Judged));
    drop(store);
    let without: String = lines.iter().map(|l| format!("{l}\n")).collect();

    // Its judgment is another finding's.
    let mut record: serde_json::Value = serde_json::from_str(&suppression).unwrap();
    record["spec"]["judgment"] =
        serde_json::from_str::<serde_json::Value>(lines[1]).unwrap()["metadata"]["name"].clone();
    let canonical = serde_json::to_string(&record["spec"]).unwrap();
    record["metadata"]["name"] = lighthouse_model::hash::short(&canonical, 16).into();
    fs::write(&path, format!("{without}{record}\n")).unwrap();
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.notices().len(), 1, "{:?}", store.notices());
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Judged));
}
