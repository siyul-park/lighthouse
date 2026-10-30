//! Looking at the decisions of a project and running their examples.

use crate::fixture::{project, session, trusted, write};

#[test]
fn decision_rows() {
    let dir = project("warn");
    let session = session(dir.path());
    let registry = session.in_process_registry().unwrap();

    let ruled = lighthouse_session::decision_rows(&session.catalog, &registry, false);
    let short = ruled.iter().find(|r| r.id == "local/short").unwrap();
    assert_eq!(short.title, "Notes are short");
    assert!(ruled.iter().any(|r| r.id == "core/max-lines"));

    let all = lighthouse_session::decision_rows(&session.catalog, &registry, true);
    assert!(all.len() >= ruled.len());
    let short = all.iter().find(|r| r.id == "local/short").unwrap();
    assert_eq!(short.severity, "warn");
    // Each decision is listed once, whether or not a rule is registered for it.
    let mut ids: Vec<_> = all.iter().map(|r| r.id.as_str()).collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(before, ids.len());
}

#[test]
fn active_decisions() {
    let dir = project("error");
    let active = lighthouse_session::active_decisions(&session(dir.path())).unwrap();
    assert!(active.contains("local/short"));

    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\", \"local\"]\n[rules]\n\"local/short\" = \"off\"\n",
        ),
    );
    let off = lighthouse_session::active_decisions(&session(dir.path())).unwrap();
    assert!(!off.contains("local/short"));
}

#[test]
fn config_summary() {
    let dir = project("error");
    let summary = lighthouse_session::config_summary(&session(dir.path())).unwrap();
    assert_eq!(summary["plugins"], serde_json::json!(["core", "local"]));
    assert!(
        summary["activeRules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "local/short")
    );
    assert!(
        summary["file"].as_str().unwrap().contains("[spec.rules]")
            || summary["file"].as_str().unwrap().contains("local/short")
    );
    assert!(summary["root"].as_str().unwrap().len() > 1);
}

#[test]
fn catalog_index() {
    let dir = project("warn");
    let index = lighthouse_session::catalog_index(&session(dir.path())).unwrap();
    let line = index
        .lines()
        .find(|l| l.starts_with("local/short\t"))
        .unwrap();
    let fields: Vec<&str> = line.split('\t').collect();
    assert_eq!(fields[1], "warn");
    assert_eq!(fields[3], "Notes are short");
    assert!(
        index.lines().count() > 10,
        "the bundled decisions are in it"
    );
}

#[test]
fn decision_text() {
    let dir = project("error");
    let session = session(dir.path());
    let text = lighthouse_session::decision_text(&session, "local/short").unwrap();
    assert!(text.contains("Notes are short"), "{text}");
    assert!(
        text.contains("A note MUST NOT run past two lines."),
        "{text}"
    );

    let unknown = lighthouse_session::decision_text(&session, "local/none")
        .err()
        .unwrap();
    assert!(
        unknown.to_string().contains("unknown decision"),
        "{unknown}"
    );
}

#[test]
fn explain() {
    let dir = project("error");
    let session = session(dir.path());
    let registry = session.in_process_registry().unwrap();

    let decision = lighthouse_session::explain(&session.catalog, &registry, "local/short").unwrap();
    assert!(decision.contains("Notes are short"), "{decision}");
    assert!(decision.contains("Status: implemented"), "{decision}");

    let unknown = lighthouse_session::explain(&session.catalog, &registry, "local/none")
        .err()
        .unwrap();
    assert!(unknown.to_string().contains("unknown decision or rule"));
}

#[test]
fn test_decisions() {
    let dir = project("error");
    let session = trusted(dir.path());

    let ids = ["local/short".to_owned()];
    let tested = lighthouse_session::test_decisions(&session, &ids, Some("text")).unwrap();
    assert_eq!(tested.decisions, 1);
    assert_eq!(tested.languages, 1);
    assert_eq!(tested.runs, 1);
    assert!(tested.failures.is_empty(), "{:?}", tested.failures);

    // An example the check does not agree with is a failure.
    let text = crate::fixture::read(dir.path(), ".lighthouse/decisions/short.yaml");
    write(
        dir.path(),
        ".lighthouse/decisions/short.yaml",
        &text.replace("file.lines > 2", "file.lines > 5"),
    );
    let failing =
        lighthouse_session::test_decisions(&trusted(dir.path()), &ids, Some("text")).unwrap();
    assert!(!failing.failures.is_empty());
    assert!(
        failing.failures[0].starts_with("[text]"),
        "{:?}",
        failing.failures
    );

    let unknown = lighthouse_session::test_decisions(&session, &["local/none".to_owned()], None);
    assert!(unknown.is_err());
}
