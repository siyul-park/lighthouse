//! Adding and replacing the decisions of a project, behind the gate: the
//! candidate loads, compiles and passes its own examples.

use lighthouse_session::{AgentKind, Attribution, Session};
use serde_json::{Value, json};

use crate::fixture::{canonical, project, read, session};

fn kim() -> Attribution {
    Attribution::new(AgentKind::Person, Some("kim".to_owned()))
}

/// A decision that fails files with a `.log` name; `suffix` is what its
/// invalid example is called, which is what the tests change.
fn document(name: &str, requirement: &str) -> Value {
    json!({
        "kind": "Decision",
        "metadata": { "name": format!("local/{name}") },
        "spec": {
            "title": "Logs are not committed",
            "context": "A log is a build product.",
            "scope": { "subject": "file" },
            "requirement": requirement,
            "severity": "warn",
            "check": {
                "type": "cel",
                "select": "file",
                "where": "file.path.endsWith(\".log\")",
                "message": "{{ file.path }} is a log"
            },
            "examples": [
                { "name": "log", "language": "text", "kind": "invalid",
                  "files": [{ "path": "x.log", "body": "x" }], "expect": [{ "line": 1 }] },
                { "name": "text", "language": "text", "kind": "valid",
                  "files": [{ "path": "x.txt", "body": "x" }] }
            ]
        }
    })
}

fn opened(dir: &std::path::Path) -> Session {
    session(&canonical(dir))
}

#[test]
fn create_decision() {
    let dir = project("error");
    let session = opened(dir.path());
    let made = lighthouse_session::create_decision(
        &session,
        document("no-logs", "A log MUST NOT be committed."),
        &kim(),
    )
    .unwrap();
    assert_eq!(made.id, "local/no-logs");
    assert!(made.created);
    assert_eq!((made.test.decisions, made.test.runs), (1, 1));
    assert!(made.test.failures.is_empty(), "{:?}", made.test.failures);
    assert!(made.local_plugin_listed);

    // It is a proposal with a uid, and a person accepts it.
    let written = read(dir.path(), ".lighthouse/decisions/no-logs.yaml");
    assert!(written.contains("status: proposed"), "{written}");
    assert!(written.contains("uid:"), "{written}");
    assert!(
        opened(dir.path())
            .catalog
            .decision("local/no-logs")
            .is_some()
    );

    let again = lighthouse_session::create_decision(
        &opened(dir.path()),
        document("no-logs", "A log MUST NOT be committed."),
        &kim(),
    );
    assert!(again.err().unwrap().to_string().contains("already exists"));

    // The same wording under another name is reported as a possible repeat,
    // and written all the same: a person decides.
    let repeat = lighthouse_session::create_decision(
        &opened(dir.path()),
        document("no-log-files", "A log MUST NOT be committed."),
        &kim(),
    )
    .unwrap();
    assert!(repeat.created);
    assert_eq!(repeat.duplicates.len(), 1, "{:?}", repeat.duplicates);
    assert_eq!(repeat.duplicates[0].id, "local/no-logs");

    for refused in [
        json!({ "kind": "Decision", "metadata": { "name": "core/other" }, "spec": {} }),
        json!({ "kind": "Decision", "metadata": { "name": "local/../x" }, "spec": {} }),
        json!({ "kind": "Project", "metadata": { "name": "local/x" } }),
    ] {
        assert!(lighthouse_session::create_decision(&opened(dir.path()), refused, &kim()).is_err());
    }
}

#[test]
fn a_decision_whose_examples_fail_is_not_written() {
    let dir = project("error");
    let mut bad = document("no-logs", "A log MUST NOT be committed.");
    bad["spec"]["examples"][1]["files"][0]["path"] = json!("x.log");
    let refused = lighthouse_session::create_decision(&opened(dir.path()), bad, &kim());
    assert!(refused.is_err());
    assert!(
        !dir.path()
            .join(".lighthouse/decisions/no-logs.yaml")
            .exists()
    );
}

#[test]
fn replace_decision() {
    let dir = project("error");
    lighthouse_session::create_decision(
        &opened(dir.path()),
        document("no-logs", "A log MUST NOT be committed."),
        &kim(),
    )
    .unwrap();
    let uid = read(dir.path(), ".lighthouse/decisions/no-logs.yaml")
        .lines()
        .find(|l| l.trim_start().starts_with("uid:"))
        .unwrap()
        .to_owned();

    let replaced = lighthouse_session::replace_decision(
        &opened(dir.path()),
        document("no-logs", "A log MUST NOT enter the repository."),
        &kim(),
    )
    .unwrap();
    assert!(!replaced.created);
    let written = read(dir.path(), ".lighthouse/decisions/no-logs.yaml");
    assert!(
        written.contains("MUST NOT enter the repository"),
        "{written}"
    );
    assert!(written.contains(&uid), "the uid is kept: {written}");

    let missing = lighthouse_session::replace_decision(
        &opened(dir.path()),
        document("no-such", "A log MUST NOT be committed."),
        &kim(),
    );
    assert!(
        missing
            .err()
            .unwrap()
            .to_string()
            .contains("not a local decision")
    );
    let bundled = lighthouse_session::replace_decision(
        &opened(dir.path()),
        json!({ "kind": "Decision", "metadata": { "name": "core/max-lines" }, "spec": {} }),
        &kim(),
    );
    assert!(
        bundled
            .err()
            .unwrap()
            .to_string()
            .contains("not a local decision")
    );
}
