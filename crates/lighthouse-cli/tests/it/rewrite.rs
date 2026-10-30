//! One rewrite path: a command that changes files and an agent are run in the
//! same scratch copy of the project and judged by the same change collection,
//! scope check and outcome mapping. Both are small shell scripts here, so no
//! real tool or agent is run.

use std::fs;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{cache_fixtures::write, mcp::Client};

const ID: &str = "local/pair";

/// A decision that flags `a.txt` while it has more than one line, with `fix`
/// among its lines.
fn pair(fix: &str) -> String {
    format!(
        r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: {ID}
spec:
  title: A pair is kept short
  context: The first note is read in one glance.
  scope:
    subject: file
  requirement: The first note MUST NOT exceed one line.
  severity: error
  check:
    type: cel
    select: file
    where: "file.path == \"a.txt\" && file.lines > 1"
    message: "{{{{ file.path }}}} is too long"
    evidence:
      path: file.path
{fix}  examples:
    - name: long
      language: text
      kind: invalid
      files:
        - path: a.txt
          body: "one\ntwo\n"
        - path: b.txt
          body: "b\n"
      expect:
        - line: 1
      fixed:
        - path: a.txt
          body: "one\n"
        - path: b.txt
          body: "B\n"
    - name: short
      language: text
      kind: valid
      files:
        - path: a.txt
          body: "one\n"
"#
    )
}

/// Edits the two files, as a fix whose scope is `files` may.
const BOTH: &str = "printf 'one\\n' > a.txt\nprintf 'B\\n' > b.txt\n";

/// How the change is made: by a command the decision names, or by the agent
/// the project binds.
#[derive(Clone, Copy)]
enum By {
    Command,
    Agent,
}

/// A project whose decision is fixed `by` running `script` in the scratch
/// copy with `scope`; and the directory the script lives in.
fn project(by: By, scope: &str, script: &str) -> (TempDir, TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    write(scripts.path(), "change.sh", script);
    let path = scripts.path().join("change.sh");
    let program = format!("[\"sh\", {:?}]", path.to_str().unwrap());
    let (fix, models) = match by {
        By::Command => (
            format!(
                "  fix:\n    safety: unsafe\n    type: command\n    scope: {scope}\n    command: {program}\n"
            ),
            String::new(),
        ),
        By::Agent => (
            format!(
                "  fix:\n    safety: unsafe\n    type: model\n    scope: {scope}\n    prompt: Shorten it.\n"
            ),
            format!(
                "[models]\nagent = \"agent/m\"\n[models.providers.agent]\ncommand = {program}\n"
            ),
        ),
    };
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = []\n[rules]\n{ID:?} = \"error\"\n{models}[fix]\nunsafe = true\ndecisions = [{ID:?}]\n"
        )),
    );
    write(dir.path(), ".lighthouse/decisions/pair.yaml", &pair(&fix));
    write(dir.path(), "a.txt", "one\ntwo\n");
    write(dir.path(), "b.txt", "b\n");
    (dir, scripts)
}

/// What a fix run did: the project's files afterwards, and the one thing that
/// became of the finding.
struct Outcome {
    a: String,
    b: String,
    created: bool,
    applied: usize,
    declined: Option<String>,
}

fn run(by: By, scope: &str, script: &str) -> Outcome {
    let (dir, _scripts) = project(by, scope, script);
    let done: Value = Client::trusting(dir.path())
        .tool("fix", json!({"decision": [ID]}))
        .unwrap();
    let read = |name: &str| fs::read_to_string(dir.path().join(name)).unwrap();
    Outcome {
        a: read("a.txt"),
        b: read("b.txt"),
        created: dir.path().join("c.txt").exists(),
        applied: done["applied"].as_array().unwrap().len(),
        declined: done["declined"][0]["reason"].as_str().map(str::to_owned),
    }
}

/// A reason with the word for who changed the files taken out.
fn same(reason: &str) -> String {
    reason
        .replacen("the command", "the program", 1)
        .replacen("the agent", "the program", 1)
}

#[test]
fn a_command_and_an_agent_change_the_same_files_under_scope_files() {
    for by in [By::Command, By::Agent] {
        let done = run(by, "files", BOTH);

        assert_eq!(done.applied, 1, "{:?}", done.declined);
        assert_eq!((done.a.as_str(), done.b.as_str()), ("one\n", "B\n"));
        assert_eq!(done.declined, None);
    }
}

#[test]
fn a_command_and_an_agent_are_refused_the_same_way_outside_their_scope() {
    let reasons: Vec<String> = [By::Command, By::Agent]
        .into_iter()
        .map(|by| {
            let done = run(by, "file", BOTH);

            assert_eq!(done.applied, 0);
            assert_eq!((done.a.as_str(), done.b.as_str()), ("one\ntwo\n", "b\n"));
            done.declined.expect("declined")
        })
        .collect();

    for reason in &reasons {
        assert!(
            reason.contains("changed `b.txt`") && reason.contains("scope `file`"),
            "{reason}"
        );
    }
    assert_eq!(
        same(&reasons[0]),
        same(&reasons[1]),
        "one reason, whoever changed the file"
    );
}

#[test]
fn a_command_and_an_agent_may_not_create_a_file() {
    let script = format!("{BOTH}printf 'x\\n' > c.txt\n");
    let reasons: Vec<String> = [By::Command, By::Agent]
        .into_iter()
        .map(|by| {
            let done = run(by, "files", &script);

            assert!(!done.created, "the file was written to the project");
            assert_eq!(done.a, "one\ntwo\n");
            done.declined.expect("declined")
        })
        .collect();

    for reason in &reasons {
        assert!(reason.contains("created `c.txt`"), "{reason}");
    }
    assert_eq!(same(&reasons[0]), same(&reasons[1]));
}

#[test]
fn a_command_and_an_agent_that_change_nothing_are_declined_the_same_way() {
    let reasons: Vec<String> = [By::Command, By::Agent]
        .into_iter()
        .map(|by| run(by, "files", "true\n").declined.expect("declined"))
        .collect();

    for reason in &reasons {
        assert!(reason.contains("changed no file"), "{reason}");
    }
}
