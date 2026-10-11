//! Model checks end to end: a cel check whose `review` abstains to a model
//! the project binds. The model is a `command` provider, a small script, so
//! no network is used.

use std::{fs, path::Path};

use assert_cmd::Command;
use serde_json::Value;

use crate::cache_fixtures::write;

const DECISION: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/naming-ban
spec:
  title: Ban
  context: A function named by a ban word is a violation.
  scope: { subject: symbol }
  requirement: A function MUST NOT be named like a ban word.
  severity: warn
  check:
    type: cel
    where: symbol.kind == "function" && symbol.name == "bad_one"
    review: symbol.kind == "function" && symbol.name.startsWith("maybe")
    message: "{{ symbol.name }} is banned"
  examples:
    - name: bad
      language: rust
      kind: invalid
      files: [{ path: src/lib.rs, body: "pub fn bad_one() {}\n" }]
      expect: [{ line: 1 }]
    - name: good
      language: rust
      kind: valid
      files: [{ path: src/lib.rs, body: "pub fn fine() {}\n" }]
"#;

/// Answers System One questions by the source of the subject: `maybe_bad`
/// is surely a violation, `maybe_good` surely not, anything else unsure. It
/// counts its runs in `calls`.
const SCRIPT: &str = r#"import json, sys
request = json.load(sys.stdin)
open("calls", "a").write("x")
text = request["state"]["text"]
p = 0.95 if "maybe_bad" in text else 0.05 if "maybe_good" in text else 0.5
answers = {id: {"type": "noul", "noul": p} for id in request["questions"]}
print(json.dumps({"model": request["model"], "answers": answers}))
"#;

const LIB: &str = "pub fn bad_one() {}\npub fn maybe_bad() {}\npub fn maybe_good() {}\npub fn maybe_unsure() {}\npub fn fine() {}\n";

const BOUND: &str = "[models]\nclassification = \"fake/any\"\n[models.providers.fake]\ntype = \"command\"\nargv = [\"python3\", \"model.py\"]\n";

pub(crate) fn project(models: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let plugin = lighthouse_test_support::lang_rust();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"core\", \"local\"]\n[rules]\n\"local/naming-ban\" = \"warn\"\n{models}",
            plugin.to_str().unwrap()
        )),
    );
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(dir.path(), "src/lib.rs", LIB);
    write(dir.path(), "model.py", SCRIPT);
    write(dir.path(), ".lighthouse/decisions/ban.yaml", DECISION);
    dir
}

fn check(dir: &Path, args: &[&str]) -> (i32, Value) {
    let out = Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(dir)
        .env("LIGHTHOUSE_TRUST", "1")
        .args(["check", "--no-store", "--format", "sarif"])
        .args(args)
        .output()
        .unwrap();
    let log: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code().unwrap(), log)
}

/// Message, kind and level of the results of the decision, sorted.
pub(crate) fn results(log: &Value) -> Vec<(String, String, String)> {
    let mut found: Vec<_> = log["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["ruleId"] == "local/naming-ban")
        .map(|r| {
            (
                r["message"]["text"].as_str().unwrap().to_owned(),
                r["kind"].as_str().unwrap_or("fail").to_owned(),
                r["level"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    found.sort();
    found
}

pub(crate) fn count(found: &[(String, String, String)], kind: &str) -> usize {
    found.iter().filter(|r| r.1 == kind).count()
}

fn calls(dir: &Path) -> usize {
    fs::read_to_string(dir.join("calls")).map_or(0, |t| t.len())
}

#[test]
fn a_bound_model_fails_what_it_is_sure_of_and_the_rest_is_left_to_review() {
    let dir = project(BOUND);
    let (code, log) = check(dir.path(), &["--no-cache"]);
    let found = results(&log);
    assert_eq!(count(&found, "fail"), 2, "{found:?}");
    assert!(found.iter().any(|r| r.0 == "bad_one is banned"));
    assert!(found.iter().any(|r| r.0 == "maybe_bad is banned"));
    assert_eq!(count(&found, "review"), 2, "{found:?}");
    assert!(
        found
            .iter()
            .filter(|r| r.1 == "review")
            .all(|r| r.2 == "none")
    );
    assert_eq!(code, 0, "warnings do not fail the run, a review never does");
}

#[test]
fn without_a_binding_or_with_models_off_every_abstention_is_a_review() {
    for (models, args) in [("", &[][..]), (BOUND, &["--no-models"][..])] {
        let dir = project(models);
        let mut all = vec!["--no-cache"];
        all.extend(args);
        let (_, log) = check(dir.path(), &all);
        let found = results(&log);
        assert_eq!(count(&found, "fail"), 1, "{found:?}");
        assert_eq!(count(&found, "review"), 3, "{found:?}");
        assert_eq!(calls(dir.path()), 0);
    }
}

#[test]
fn the_budget_caps_what_a_model_that_costs_is_asked() {
    let dir = project(BOUND);
    let text = fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        text.replace(
            "\"classification\" =",
            "\"budget\" = 1, \"classification\" =",
        ),
    )
    .unwrap();
    let (_, log) = check(dir.path(), &["--no-cache"]);
    let found = results(&log);
    // One of the three candidates was asked; the others stay for review.
    assert_eq!(count(&found, "review"), 2, "{found:?}");
}

#[test]
fn answers_are_cached_and_a_warm_run_prints_what_a_cold_run_prints() {
    let dir = project(BOUND);
    let (_, cold) = check(dir.path(), &["--no-cache"]);
    assert_eq!(calls(dir.path()), 3);
    let (_, first) = check(dir.path(), &[]);
    let after_first = calls(dir.path());
    let (_, warm) = check(dir.path(), &[]);
    assert_eq!(calls(dir.path()), after_first, "a warm run asks nothing");
    assert_eq!(results(&cold), results(&first));
    assert_eq!(results(&cold), results(&warm));
}

#[test]
fn a_model_that_fails_leaves_its_subjects_to_review_and_the_run_complete() {
    let dir = project(BOUND);
    write(dir.path(), "model.py", "import sys\nsys.exit(3)\n");
    let (code, log) = check(dir.path(), &["--no-cache"]);
    assert_eq!(code, 0);
    assert_eq!(count(&results(&log), "review"), 3);
    assert_eq!(
        log["runs"][0]["invocations"][0]["executionSuccessful"],
        true
    );
}

#[test]
fn a_command_model_is_not_run_until_the_project_is_trusted() {
    let dir = project(BOUND);
    let out = Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(dir.path())
        .env_remove("LIGHTHOUSE_TRUST")
        .env("HOME", dir.path())
        .args(["check", "--no-store", "--no-cache", "--format", "sarif"])
        .output()
        .unwrap();
    let log: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(count(&results(&log), "review"), 3);
    assert_eq!(calls(dir.path()), 0);
}

#[test]
fn recorded_model_labels_are_judgments_attributed_to_the_model() {
    let dir = project(BOUND);
    Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args([
            "check",
            "--no-cache",
            "--record-model-labels",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let log = fs::read_to_string(dir.path().join(".lighthouse/decisions.jsonl")).unwrap();
    let judgment: Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(judgment["spec"]["judgment"], "fail");
    assert_eq!(judgment["spec"]["wasAttributedTo"]["type"], "SoftwareAgent");
    let id = judgment["spec"]["wasAttributedTo"]["id"].as_str().unwrap();
    assert!(id.starts_with("fake/any@"), "{id}");
}

/// Answers 0.5 to everything, in an envelope, as text holding fenced JSON.
const WEAK: &str = r#"import sys
sys.stdin.read()
print('{"result": "```json\\n{\\"noul\\": 0.5}\\n```"}')
"#;

const LAYERED: &str = "[models]\nclassification = \"fake/any\"\n[models.providers.fake]\ntype = \"command\"\nargv = [\"python3\", \"model.py\"]\n[models.providers.weak]\ntype = \"command\"\nargv = [\"python3\", \"weak.py\"]\nstdin = \"prompt\"\npointer = \"/result\"\n";

#[test]
fn an_abstention_falls_through_the_layers_and_a_prompt_command_answers_through_a_pointer() {
    let dir = project(LAYERED);
    write(dir.path(), "weak.py", WEAK);
    let text = fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        text.replace(
            "\"local/naming-ban\" = \"warn\"",
            "\"local/naming-ban\" = { level = \"warn\", model = \"weak/m\" }",
        ),
    )
    .unwrap();
    let (_, log) = check(dir.path(), &["--no-cache"]);
    let found = results(&log);
    // The rule's own model is unsure of everything, so the project's decides.
    assert_eq!(count(&found, "fail"), 2, "{found:?}");
    assert_eq!(count(&found, "review"), 2, "{found:?}");
}
