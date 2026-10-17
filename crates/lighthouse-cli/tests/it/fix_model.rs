//! Fixes by a model through the binary: the `fix` tool and `check --fix`
//! against a project that binds a `chat` model. The model is the `fake-model`
//! plugin, which rewrites the code it is shown by a fixed rule, or a command
//! that prints an answer, so no network is used.

use std::{fs, path::Path};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{
    cache_fixtures::write,
    mcp::Client,
    models::project,
    models_plugin::{lighthouse, with_plugin},
};

const LIB: &str = "pub fn bad_one() {}\npub fn fine() {}\n";

const WORSE: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/naming-worse
spec:
  title: Worse
  context: A function named worse is worse.
  scope: { subject: symbol }
  requirement: A function MUST NOT be named worse.
  severity: warn
  check:
    type: cel
    where: symbol.kind == "function" && symbol.name == "worse"
    message: "{{ symbol.name }} is worse"
  examples:
    - name: bad
      language: rust
      kind: invalid
      files: [{ path: src/lib.rs, body: "pub fn worse() {}\n" }]
      expect: [{ line: 1 }]
    - name: good
      language: rust
      kind: valid
      files: [{ path: src/lib.rs, body: "pub fn fine() {}\n" }]
"#;

/// What the plugin does by default: rewrite `bad_one` to `fixed_one`.
const REWRITE: &str = "from = \"bad_one\", to = \"fixed_one\"";

/// The plugin as chat model `fake/m1` with these `options`, and `extra` as
/// further configuration.
fn fake(options: &str, extra: &str) -> TempDir {
    chatting("", options, extra)
}

/// The same, with `rules` among the rules of the project.
fn chatting(rules: &str, options: &str, extra: &str) -> TempDir {
    let models = format!(
        "{rules}[models]\nchat = \"fake/m1\"\n[models.providers.fake]\nplugin = \"fake-model\"\noptions = {{ {options} }}\n{extra}"
    );
    let dir = project(&models);
    with_plugin(dir.path());
    write(dir.path(), "src/lib.rs", LIB);
    dir
}

const VERIFIED: &str = "[approval]\nfix = \"verified\"\n";

fn lib(dir: &TempDir) -> String {
    fs::read_to_string(dir.path().join("src/lib.rs")).unwrap()
}

/// The `fix` tool over the decision of the project.
fn fix(dir: &Path, arguments: Value) -> Value {
    let mut arguments = arguments;
    arguments["decision"] = json!(["local/naming-ban"]);
    Client::trusting(dir).tool("fix", arguments).unwrap()
}

fn counts(fixed: &Value) -> &Value {
    &fixed["models"]["local/naming-ban"]
}

#[test]
fn a_verified_fix_by_a_model_is_applied_when_the_project_approves() {
    let dir = fake(REWRITE, VERIFIED);

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), "pub fn fixed_one() {}\npub fn fine() {}\n");
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
    let applied = &fixed["applied"][0];
    assert_eq!(applied["decision"], "local/naming-ban");
    assert_eq!(applied["fixer"], "lighthouse/model");
    assert_eq!(applied["safety"], "suggested");
    assert_eq!(applied["description"], "model fix by `fake/m1`");
    assert_eq!(
        counts(&fixed),
        &json!({"asked": 1, "answered": 1, "verified": 1, "applied": 1, "previewed": 0, "declined": {}})
    );
    assert_eq!(fixed["usage"]["calls"], 1);
    assert!(fixed.get("previews").is_none(), "{fixed}");
}

#[test]
fn without_approval_a_verified_fix_is_a_preview_and_the_file_is_left_alone() {
    let dir = fake(REWRITE, "");

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), LIB);
    assert!(fixed["applied"].as_array().unwrap().is_empty(), "{fixed}");
    let preview = &fixed["previews"][0];
    assert_eq!(preview["decision"], "local/naming-ban");
    assert_eq!(preview["safety"], "suggested");
    assert_eq!(preview["files"][0], "src/lib.rs");
    let diff = preview["diff"].as_str().unwrap();
    assert!(
        diff.contains("-pub fn bad_one() {}") && diff.contains("+pub fn fixed_one() {}"),
        "{diff}"
    );
    assert_eq!(counts(&fixed)["verified"], 1);
    assert_eq!(counts(&fixed)["previewed"], 1);
    assert_eq!(counts(&fixed)["applied"], 0);
}

#[test]
fn unsafe_fixes_applies_a_verified_fix_by_a_model_whatever_the_approval() {
    let dir = fake(REWRITE, "");

    let fixed = fix(dir.path(), json!({"unsafeFixes": true}));

    assert_eq!(lib(&dir), "pub fn fixed_one() {}\npub fn fine() {}\n");
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
    assert!(fixed.get("previews").is_none(), "{fixed}");
}

#[test]
fn a_dry_run_writes_nothing_and_shows_what_would_be_written() {
    let dir = fake(REWRITE, VERIFIED);

    let fixed = fix(dir.path(), json!({"dryRun": true}));

    assert_eq!(lib(&dir), LIB);
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
    assert!(
        fixed["diff"]
            .as_str()
            .unwrap()
            .contains("+pub fn fixed_one() {}")
    );
}

#[test]
fn a_replacement_that_leaves_the_finding_is_declined_as_not_resolving_it() {
    let dir = fake("from = \"{}\", to = \"{ }\"", VERIFIED);

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), LIB);
    assert!(fixed["applied"].as_array().unwrap().is_empty(), "{fixed}");
    assert_eq!(
        fixed["declined"][0]["reason"],
        "did not resolve the finding"
    );
    assert_eq!(counts(&fixed)["verified"], 0);
    assert_eq!(counts(&fixed)["declined"], json!({"unresolved": 1}));
}

#[test]
fn a_replacement_that_adds_a_warning_is_declined_as_introducing_it() {
    let dir = chatting(
        "\"local/naming-worse\" = \"warn\"\n",
        "from = \"bad_one\", to = \"worse\"",
        VERIFIED,
    );
    write(dir.path(), ".lighthouse/decisions/worse.yaml", WORSE);

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), LIB);
    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert_eq!(reason, "introduced local/naming-worse at line 1", "{fixed}");
    assert_eq!(counts(&fixed)["declined"], json!({"introduced": 1}));
}

#[test]
fn an_answer_that_does_not_parse_is_declined() {
    let dir = fake("answer = \"I would rename it.\"", VERIFIED);

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), LIB);
    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("did not parse"), "{reason}");
    assert_eq!(counts(&fixed)["answered"], 1);
    assert_eq!(counts(&fixed)["declined"], json!({"unparsed": 1}));
}

#[test]
fn a_failing_verify_command_withdraws_the_fixes_by_a_model_to_previews() {
    let verify =
        format!("{VERIFIED}[fix]\nverify = [[\"sh\", \"-c\", \"echo tests broke >&2; exit 1\"]]\n");
    let dir = fake(REWRITE, &verify);

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), LIB);
    assert!(fixed["applied"].as_array().unwrap().is_empty(), "{fixed}");
    assert_eq!(fixed["previews"].as_array().unwrap().len(), 1, "{fixed}");
    let messages = fixed["messages"].to_string();
    assert!(
        messages.contains("fixes by a model are previews")
            && messages.contains("exited 1")
            && messages.contains("tests broke"),
        "{messages}"
    );
    assert_eq!(counts(&fixed)["previewed"], 1);
}

#[test]
fn a_passing_verify_command_runs_over_the_fixed_texts_and_the_fix_is_written() {
    let verify =
        format!("{VERIFIED}[fix]\nverify = [[\"sh\", \"-c\", \"grep -q fixed_one src/lib.rs\"]]\n");
    let dir = fake(REWRITE, &verify);

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
    assert_eq!(lib(&dir), "pub fn fixed_one() {}\npub fn fine() {}\n");
}

#[test]
fn a_second_run_takes_the_answer_from_the_cache_and_asks_no_model() {
    let marks = tempfile::tempdir().unwrap();
    let calls = marks.path().join("calls");
    let dir = fake(
        &format!("{REWRITE}, calls = {:?}", calls.to_str().unwrap()),
        "",
    );

    let first = fix(dir.path(), json!({}));
    assert_eq!(first["usage"]["calls"], 1, "{first}");
    assert_eq!(fs::read_to_string(&calls).unwrap().len(), 1);

    let second = fix(dir.path(), json!({}));

    assert_eq!(second["usage"]["calls"], 0, "{second}");
    assert_eq!(
        fs::read_to_string(&calls).unwrap().len(),
        1,
        "no second call"
    );
    assert_eq!(second["previews"], first["previews"]);
}

#[test]
fn a_plain_text_command_serves_chat() {
    let dir = project(&format!(
        "[models]\nchat = \"writer/any\"\n[models.providers.writer]\ncommand = [\"cat\", \"answer.txt\"]\n{VERIFIED}"
    ));
    write(dir.path(), "src/lib.rs", LIB);
    write(
        dir.path(),
        "answer.txt",
        "Sure: {\"replacement\": \"pub fn fixed_one() {}\"}\n",
    );

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), "pub fn fixed_one() {}\npub fn fine() {}\n");
    assert_eq!(
        fixed["applied"][0]["description"],
        "model fix by `writer/any`"
    );
}

#[test]
fn an_openai_command_serves_chat() {
    let dir = project(&format!(
        "[models]\nchat = \"writer/any\"\n[models.providers.writer]\nprotocol = \"openai\"\ncommand = [\"cat\", \"answer.json\"]\n{VERIFIED}"
    ));
    write(dir.path(), "src/lib.rs", LIB);
    let content = json!({"replacement": "pub fn fixed_one() {}"}).to_string();
    let response = json!({
        "id": "x", "object": "chat.completion", "created": 0, "model": "m",
        "choices": [{"index": 0, "finish_reason": "stop",
                     "message": {"role": "assistant", "content": content}}]
    });
    write(dir.path(), "answer.json", &response.to_string());

    let fixed = fix(dir.path(), json!({}));

    assert_eq!(lib(&dir), "pub fn fixed_one() {}\npub fn fine() {}\n");
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
}

#[test]
fn a_provider_that_does_not_serve_chat_cannot_be_bound_to_it() {
    let dir = project(
        "[models]\nchat = \"fake/any\"\n[models.providers.fake]\nprotocol = \"typesafe\"\ncommand = [\"cat\"]\n",
    );

    let out = lighthouse(dir.path(), &["check", "--no-store", "--no-cache"]);

    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not serve `chat`"), "{stderr}");
}

#[test]
fn without_a_chat_binding_a_fix_run_is_what_it_was() {
    let dir = project("");
    write(dir.path(), "src/lib.rs", LIB);

    let fixed = fix(dir.path(), json!({}));

    for key in ["previews", "models", "usage"] {
        assert!(fixed.get(key).is_none(), "{key} in {fixed}");
    }
    assert!(fixed["applied"].as_array().unwrap().is_empty());
    assert_eq!(lib(&dir), LIB);
}

#[test]
fn check_fix_tells_what_became_of_the_findings_given_to_a_model() {
    let dir = fake(REWRITE, VERIFIED);

    let out = lighthouse(
        dir.path(),
        &[
            "check",
            "--fix",
            "--no-store",
            "--no-cache",
            "--format",
            "text",
        ],
    );

    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("model fixes local/naming-ban: asked 1, answered 1, verified 1, applied 1, previewed 0, declined 0"),
        "{text}"
    );
    assert!(text.contains("models: 1 request(s)"), "{text}");
    assert_eq!(lib(&dir), "pub fn fixed_one() {}\npub fn fine() {}\n");
}

#[test]
fn with_models_off_no_model_is_asked_to_fix() {
    let dir = fake(REWRITE, VERIFIED);

    let out = lighthouse(
        dir.path(),
        &["check", "--fix", "--no-models", "--no-store", "--no-cache"],
    );

    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(!text.contains("model fixes"), "{text}");
    assert_eq!(lib(&dir), LIB);
}
