//! Adding and replacing the decisions of a project, behind the gate: the
//! candidate loads, compiles and passes its own examples.

use lighthouse_session::{AgentKind, Attribution, Extended, Session, ensure_extends};
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

#[test]
fn create_decision_has_the_project_extend_core_authoring_once() {
    let dir = project("error");
    let made = lighthouse_session::create_decision(
        &opened(dir.path()),
        document("no-logs", "A log MUST NOT be committed."),
        &kim(),
    )
    .unwrap();

    assert!(
        made.note.contains("added `core/authoring` to `extends`"),
        "{}",
        made.note
    );
    let config = read(dir.path(), "lighthouse.toml");
    assert_eq!(config.matches("core/authoring").count(), 1, "{config}");

    let second = lighthouse_session::create_decision(
        &opened(dir.path()),
        document("no-tmp", "A temporary file MUST NOT be committed."),
        &kim(),
    )
    .unwrap();

    assert!(!second.note.contains("core/authoring"), "{}", second.note);
    let config = read(dir.path(), "lighthouse.toml");
    assert_eq!(config.matches("core/authoring").count(), 1, "{config}");
}

#[test]
fn ensure_extends_adds_a_project_to_a_toml_file_and_keeps_the_rest() {
    let dir = project("error");
    let root = dir.path();
    let before = read(root, "lighthouse.toml");

    let added = ensure_extends(root, "core/authoring").unwrap();
    let again = ensure_extends(root, "core/authoring").unwrap();

    assert_eq!(added, Extended::Added(root.join("lighthouse.toml")));
    assert_eq!(again, Extended::Present);
    let after = read(root, "lighthouse.toml");
    assert!(after.contains("extends = [\"core/authoring\"]"), "{after}");
    assert!(after.contains("\"local/short\""), "the rest stays: {after}");
    assert_ne!(before, after);
}

#[test]
fn ensure_extends_leaves_a_file_that_is_not_toml_to_the_person() {
    let dir = project("error");
    let root = dir.path();
    std::fs::remove_file(root.join("lighthouse.toml")).unwrap();
    std::fs::write(
        root.join("lighthouse.yaml"),
        "apiVersion: lighthouse/v1alpha1\nkind: Project\nmetadata: {name: t}\nspec: {}\n",
    )
    .unwrap();

    let done = ensure_extends(root, "core/authoring").unwrap();

    assert_eq!(done, Extended::Manual(root.join("lighthouse.yaml")));
}
