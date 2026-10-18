//! Model checks end to end: a cel check whose `abstain` abstains to a model
//! the project binds. The model is a `command` provider, a small script, so
//! no network is used.

use std::{fs, path::Path};

use assert_cmd::Command;
use serde_json::Value;

use crate::{cache_fixtures::write, checks::home};

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
    abstain: symbol.kind == "function" && symbol.name.startsWith("maybe")
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

const LIB: &str = "pub fn bad_one() {}\npub fn maybe_bad() {}\npub fn maybe_good() {}\npub fn maybe_unsure() {}\npub fn fine() {}\n";

/// The `fake-model` binary, which is also a `command` provider.
fn fake_model() -> String {
    let dir = lighthouse_test_support::fake_model();
    dir.join("fake-model").to_str().unwrap().to_owned()
}

/// A project binding `fake/any` to the command provider in `mode`.
pub(crate) fn provider(mode: &str) -> String {
    format!(
        "[models]\nclassification = \"fake/any\"\n[models.providers.fake]\nprotocol = \"typesafe\"\ncommand = [{:?}, {mode:?}]\n",
        fake_model()
    )
}

/// The bound model: answers by the source of the subject.
pub(crate) fn bound() -> String {
    provider("command")
}

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
    write(dir.path(), ".lighthouse/decisions/ban.yaml", DECISION);
    dir
}

fn check(dir: &Path, args: &[&str]) -> (i32, Value) {
    let out = Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(dir)
        .envs(home(dir))
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
    let dir = project(&bound());
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
fn without_a_binding_or_with_models_off_a_review_route_is_skipped_unless_all_models_are_asked() {
    let cases = [
        (String::new(), &[][..], 0),
        (bound(), &["--no-models"][..], 0),
        (String::new(), &["--all-models"][..], 3),
        (bound(), &["--no-models", "--all-models"][..], 3),
    ];
    for (models, args, reviews) in cases {
        let dir = project(&models);
        let mut all = vec!["--no-cache"];
        all.extend(args);
        let (_, log) = check(dir.path(), &all);
        let found = results(&log);
        assert_eq!(count(&found, "fail"), 1, "{found:?}");
        assert_eq!(count(&found, "review"), reviews, "{found:?}");
        assert_eq!(calls(dir.path()), 0);
    }
}

#[test]
fn an_unbound_cached_run_reports_what_a_cold_run_reports() {
    let dir = project("");
    let (_, cold) = check(dir.path(), &["--no-cache"]);
    let (_, filled) = check(dir.path(), &[]);
    let (_, warm) = check(dir.path(), &[]);
    assert_eq!(count(&results(&cold), "fail"), 1);
    assert_eq!(results(&filled), results(&cold));
    assert_eq!(results(&warm), results(&cold));
    assert_eq!(count(&results(&warm), "review"), 0);
}

#[test]
fn the_budget_caps_what_a_model_that_costs_is_asked() {
    let dir = project(&bound());
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
    // One request was sent, for one of the three candidates (which one
    // depends on how the files are scheduled); the others stay for review,
    // and so does the one asked unless the model failed it.
    assert_eq!(calls(dir.path()), 1);
    assert!(count(&found, "review") >= 2, "{found:?}");
    assert_eq!(found.len(), 4, "{found:?}");
}

#[test]
fn answers_are_cached_and_a_warm_run_prints_what_a_cold_run_prints() {
    let dir = project(&bound());
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
    let dir = project(&provider("fail"));
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
    let dir = project(&bound());
    let out = Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(dir.path())
        .envs(home(dir.path()))
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
    let dir = project(&bound());
    Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(dir.path())
        .envs(home(dir.path()))
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

/// The bound model plus a weaker one that answers through a pointer.
fn layered() -> String {
    format!(
        "{}[models.providers.weak]\nprotocol = \"typesafe\"\ncommand = [{:?}, \"weak\"]\npointer = \"/result\"\n",
        bound(),
        fake_model()
    )
}

#[test]
fn an_abstention_falls_through_the_layers_and_a_command_answers_through_a_pointer() {
    let dir = project(&layered());
    let text = fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        text.replace(
            "\"local/naming-ban\" = \"warn\"",
            "\"local/naming-ban\" = { level = \"warn\", classification = \"weak/m\" }",
        ),
    )
    .unwrap();
    let (_, log) = check(dir.path(), &["--no-cache"]);
    let found = results(&log);
    // The rule's own model is unsure of everything, so the project's decides.
    assert_eq!(count(&found, "fail"), 2, "{found:?}");
    assert_eq!(count(&found, "review"), 2, "{found:?}");
}

/// A route of command models, each `(name, mode)` marking `<name>-calls`.
fn route(models: &[(&str, &str)], routing: &str) -> String {
    let names: Vec<String> = models
        .iter()
        .map(|(name, _)| format!("\"{name}/m\""))
        .collect();
    let mut text = format!(
        "[models]\nclassification = [{}]\nrouting = {routing:?}\n",
        names.join(", ")
    );
    for (name, mode) in models {
        text.push_str(&format!(
            "[models.providers.{name}]\nprotocol = \"typesafe\"\ncommand = [{:?}, {mode:?}, \"{name}-calls\"]\n",
            fake_model()
        ));
    }
    text
}

/// A project of `route`, with `extra` more subjects the models are unsure of.
fn routed(models: &[(&str, &str)], routing: &str, extra: usize) -> tempfile::TempDir {
    let dir = project(&route(models, routing));
    let more: String = (0..extra)
        .map(|n| format!("pub fn maybe_{n}() {{}}\n"))
        .collect();
    write(dir.path(), "src/more.rs", &more);
    write(dir.path(), "src/lib.rs", &format!("{LIB}mod more;\n"));
    dir
}

fn marks(dir: &Path, name: &str) -> usize {
    fs::read_to_string(dir.join(format!("{name}-calls"))).map_or(0, |t| t.len())
}

/// The run's report as JSON; one worker, so that requests to a model that
/// fails are counted in the order they are made.
fn routed_check(dir: &Path) -> Value {
    let out = Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(dir)
        .envs(home(dir))
        .env("LIGHTHOUSE_TRUST", "1")
        .env("RAYON_NUM_THREADS", "1")
        .args([
            "check",
            "--no-store",
            "--no-cache",
            "--format",
            "json",
            "--detail",
            "full",
        ])
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn a_request_moves_to_the_next_model_of_a_route_when_a_model_fails_and_a_failing_model_is_given_up()
{
    let dir = routed(&[("bad", "fail"), ("good", "command")], "ordered", 4);
    let report = routed_check(dir.path());
    let notices = report["messages"].to_string();
    assert!(notices.contains("model bad/m failed"), "{notices}");
    assert!(notices.contains("bad/m failed 3 times"), "{notices}");
    // Three subjects asked and failed; the four others skip it.
    assert_eq!(marks(dir.path(), "bad"), 3);
    assert_eq!(marks(dir.path(), "good"), 7);
    let decided: Vec<_> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["evidence"]["model"].as_str())
        .collect();
    assert_eq!(decided.len(), 1, "{report}");
    assert!(decided[0].starts_with("good/m@"), "{decided:?}");
}

#[test]
fn round_robin_starts_each_request_at_the_next_model() {
    let dir = routed(&[("a", "command"), ("b", "command")], "round-robin", 1);
    routed_check(dir.path());
    assert_eq!((marks(dir.path(), "a"), marks(dir.path(), "b")), (2, 2));
}

#[test]
fn an_abstaining_model_does_not_fall_back_to_the_next_of_its_route() {
    let models = format!(
        "[models]\nclassification = [\"weak/m\", \"good/m\"]\n\
         [models.providers.weak]\nprotocol = \"typesafe\"\ncommand = [{fake:?}, \"weak\"]\npointer = \"/result\"\n\
         [models.providers.good]\nprotocol = \"typesafe\"\ncommand = [{fake:?}, \"command\", \"good-calls\"]\n",
        fake = fake_model()
    );
    let dir = project(&models);
    let (_, log) = check(dir.path(), &["--no-cache"]);
    let found = results(&log);
    assert_eq!(count(&found, "fail"), 1, "{found:?}");
    assert_eq!(count(&found, "review"), 3, "{found:?}");
    assert_eq!(marks(dir.path(), "good"), 0);
}
