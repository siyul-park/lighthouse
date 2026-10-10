//! The decision log: what is written, what is read back, and the shapes of
//! the entries from before the resource model.

use std::fs;

use crate::support::*;
use lighthouse_model::{Reason, Verdict};
use lighthouse_store::{Error, Observed, Standing, Store};
use tempfile::TempDir;

/// A project root that has recorded `findings` and judged each one in `rejected`.
fn clone_with(findings: &[Observed], rejected: &[(&str, Reason)]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.record(&run(findings.to_vec())).unwrap();
    for (fingerprint, reason) in rejected {
        judge(&mut store, fingerprint, Verdict::Rejected, *reason);
    }
    dir
}

#[test]
fn a_decision_log_carries_suppression_to_another_clone() {
    let findings = [
        observed("f1", "design/a", "a.go"),
        observed("f2", "design/a", "b.go"),
    ];
    let author = clone_with(&findings, &[("f1", Reason::IntentionalException)]);
    assert_eq!(log_of(author.path()).lines().count(), 1);

    let teammate = tempfile::tempdir().unwrap();
    fs::create_dir_all(teammate.path().join(".lighthouse")).unwrap();
    fs::write(Store::log_path_in(teammate.path()), log_of(author.path())).unwrap();
    let mut store = Store::open_existing(teammate.path()).unwrap().unwrap();
    store.record(&run(findings.to_vec())).unwrap();
    assert_eq!(standing_of(&store, "f1"), Some(Standing::Suppressed));
    assert_eq!(standing_of(&store, "f2"), None);
    let history = store.history("f1").unwrap();
    assert_eq!(history[0].reason, Reason::IntentionalException);
    assert_eq!(history[0].reviewer_id.as_deref(), Some("claude"));
}

#[test]
fn a_verdict_is_logged_before_the_cache_and_the_cache_is_rebuilt_from_the_log() {
    let findings = [observed("f1", "design/a", "a.go")];
    let dir = clone_with(&findings, &[("f1", Reason::FalsePositive)]);
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
fn concatenated_logs_apply_every_decision_once() {
    let findings = [
        observed("f1", "design/a", "a.go"),
        observed("f2", "design/a", "b.go"),
    ];
    let one = clone_with(&findings, &[("f1", Reason::FalsePositive)]);
    let two = clone_with(&findings, &[("f2", Reason::ProjectAllowed)]);
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
        "the duplicate line is one event"
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
        &[("f1", Reason::FalsePositive)],
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
            "{\"apiVersion\":\"lighthouse/v1alpha1\",\"kind\":\"Verdict\",\"metadata\":{\"name\":"
        ),
        "{line}"
    );
    assert_eq!(value["spec"]["decisionName"], "design/a");
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
        &[("f1", Reason::FalsePositive)],
    );
    let log = log_of(dir.path());
    let path = Store::log_path_in(dir.path());

    fs::write(&path, format!("{log}not json\n")).unwrap();
    let error = Store::open(dir.path()).err().unwrap();
    assert!(matches!(error, Error::Log { line: 2, .. }), "{error}");

    fs::write(
        &path,
        log.replace(
            "\"reason\":\"false-positive\"",
            "\"reason\":\"project-allowed\"",
        ),
    )
    .unwrap();
    let error = Store::open(dir.path()).err().unwrap();
    assert!(
        error.to_string().contains("the id does not match"),
        "{error}"
    );

    let bad_pair = log.replace("\"verdict\":\"rejected\"", "\"verdict\":\"confirmed\"");
    fs::write(&path, bad_pair).unwrap();
    assert!(Store::open(dir.path()).is_err());
}

#[test]
fn an_append_after_a_line_without_a_newline_starts_a_new_line() {
    let dir = clone_with(
        &[observed("f1", "design/a", "a.go")],
        &[("f1", Reason::FalsePositive)],
    );
    let path = Store::log_path_in(dir.path());
    let log = log_of(dir.path());
    fs::write(&path, log.trim_end()).unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    judge(&mut store, "f1", Verdict::Deferred, Reason::Unspecified);
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

/// A line as builds before the resource model wrote it: one flat event with
/// snake_case keys and no `kind`.
const LEGACY_LINE: &str = r#"{"catalog_version":"cat0","commit":"abc","evidence_digest":"d1","fingerprint":"legacyf","id":"IDPLACEHOLDER","language":"go","lighthouse_version":"0.0.9","pattern_hash":"full0","reason":"false-positive","reviewer_id":"ana","reviewer_kind":"human","rule_id":"design/a","rule_version":"sem1","scope":"symbol","snapshot":{"evidence":{"fan_out":14},"severity":"review","tier":"judgment"},"timestamp":"2026-01-02T00:00:00.000Z","verdict":"rejected"}"#;

/// The id such a line has: the hash of the line without it, keys sorted.
fn legacy_line() -> String {
    let mut body: serde_json::Value = serde_json::from_str(LEGACY_LINE).unwrap();
    body.as_object_mut().unwrap().remove("id");
    let text = lighthouse_store_digest(&body);
    LEGACY_LINE.replace("IDPLACEHOLDER", &text)
}

fn lighthouse_store_digest(body: &serde_json::Value) -> String {
    let canonical = serde_json::to_string(body).unwrap();
    lighthouse_model::hash::short(&canonical, 16)
}

#[test]
fn a_log_in_the_flat_shape_from_before_the_resource_model_is_still_read() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".lighthouse")).unwrap();
    let line = legacy_line();
    fs::write(Store::log_path_in(dir.path()), format!("{line}\n")).unwrap();

    let store = Store::open(dir.path()).unwrap();

    let history = store.history("legacyf").unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].decision_hash.as_deref(), Some("full0"));
    assert_eq!(
        history[0].snapshot["severity"], "review",
        "history keeps what was recorded"
    );
    assert_eq!(
        log_of(dir.path()),
        format!("{line}\n"),
        "history is never rewritten"
    );
}

#[test]
fn old_lines_and_new_lines_live_in_one_log() {
    let dir = clone_with(
        &[observed("f1", "design/a", "a.go")],
        &[("f1", Reason::FalsePositive)],
    );
    let path = Store::log_path_in(dir.path());
    let new = log_of(dir.path());
    fs::write(&path, format!("{}\n{new}", legacy_line())).unwrap();

    let store = Store::open(dir.path()).unwrap();

    assert_eq!(store.history("legacyf").unwrap().len(), 1);
    assert_eq!(store.history("f1").unwrap().len(), 1);
}

#[test]
fn records_of_a_kind_or_version_this_build_does_not_know_are_skipped() {
    let dir = clone_with(
        &[observed("f1", "design/a", "a.go")],
        &[("f1", Reason::FalsePositive)],
    );
    let path = Store::log_path_in(dir.path());
    let log = log_of(dir.path());
    let future = r#"{"apiVersion":"lighthouse/v1alpha1","kind":"Signal","metadata":{"name":"s1"},"spec":{"anything":[1,2]}}"#;
    let newer = log.replace("v1alpha1", "v9");
    fs::write(&path, format!("{future}\n{log}{newer}")).unwrap();

    let store = Store::open(dir.path()).unwrap();

    assert_eq!(store.history("f1").unwrap().len(), 1);
    assert_eq!(store.notices().len(), 1, "{:?}", store.notices());
    assert!(
        store.notices()[0].starts_with("2 record(s)"),
        "{:?}",
        store.notices()
    );
}

#[test]
fn a_verdict_with_a_field_this_build_does_not_know_is_still_read() {
    let dir = clone_with(
        &[observed("f1", "design/a", "a.go")],
        &[("f1", Reason::FalsePositive)],
    );
    let path = Store::log_path_in(dir.path());
    let mut record: serde_json::Value = serde_json::from_str(log_of(dir.path()).trim()).unwrap();
    record["spec"]["futureField"] = serde_json::json!({ "from": "a newer build" });
    record["metadata"]["name"] = lighthouse_store_digest(&record["spec"]).into();
    fs::write(&path, format!("{record}\n")).unwrap();
    fs::remove_file(Store::path_in(dir.path())).unwrap();

    let store = Store::open(dir.path()).unwrap();

    assert_eq!(store.history("f1").unwrap().len(), 1);
    assert!(store.notices().is_empty());
}

#[test]
fn to_json_shows_a_verdict_in_camel_case_with_its_id_and_label() {
    let dir = clone_with(
        &[observed("f1", "design/a", "a.go")],
        &[("f1", Reason::FalsePositive)],
    );
    let store = Store::open(dir.path()).unwrap();
    let event = store.history("f1").unwrap().remove(0);

    let json = event.to_json();

    assert_eq!(json["ruleId"], "design/a");
    assert_eq!(json["reviewerKind"], "agent");
    assert_eq!(json["id"], event.id.as_str());
    assert_eq!(json["label"], "negative");
    assert!(json.get("rule_id").is_none());
}

#[test]
fn the_verdict_record_has_a_schema() {
    let kinds: Vec<_> = lighthouse_store::descriptors()
        .iter()
        .map(|d| d.kind)
        .collect();
    assert_eq!(kinds, ["Verdict", "Rewrite"]);
}

#[test]
fn a_verdict_record_converts_to_the_event_it_was_written_from() {
    let mut store = memory_with(&[observed("f1", "design/a", "a.go")]);
    let event = store
        .resolve(
            &review("f1", Verdict::Confirmed, Reason::Fixed),
            stamp("sem1"),
        )
        .unwrap()
        .event;

    let spec = lighthouse_store::VerdictSpec::from(&event);
    let back = lighthouse_store::ReviewEvent::from(spec.clone());

    assert_eq!(spec.decision_name, "design/a");
    assert_eq!(spec.decision_hash.as_deref(), Some("full1"));
    assert_eq!(back.fingerprint, event.fingerprint);
    assert_eq!(back.snapshot, event.snapshot);
    assert_eq!(
        back.id, "",
        "the id is the record's name, not part of its spec"
    );
}
