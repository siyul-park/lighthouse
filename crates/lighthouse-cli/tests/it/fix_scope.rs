//! Who may have a finding fixed by a model, and how far the change may go:
//! `[fix] decisions`, the scope of the fix and the brief of a review task.
//! The chain of runs of a pass is in `fix_chain`. The agent is a small shell
//! script that edits the files of its working directory, so no real agent is
//! run.

use std::fs;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{
    cache_fixtures::write,
    mcp::Client,
    models::{UNSANDBOXED, project},
    models_plugin::{fingerprints, lighthouse},
};

pub(crate) const BAN: &str = "local/naming-ban";

/// The decision of `models.rs` with the fix block of a model that may go to
/// any file.
pub(crate) const WIDE: &str = r#"apiVersion: lighthouse/v1alpha1
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
    message: "{{ symbol.name }} is banned"
  fix:
    type: model
    safety: unsafe
    scope: files
    prompt: Rename it; update the callers.
  examples:
    - name: bad
      language: rust
      kind: invalid
      files: [{ path: src/lib.rs, body: "pub fn bad_one() {}\n" }]
      fixed: [{ path: src/lib.rs, body: "pub fn fixed_one() {}\n" }]
      expect: [{ line: 1 }]
    - name: good
      language: rust
      kind: valid
      files: [{ path: src/lib.rs, body: "pub fn fine() {}\n" }]
"#;

/// The file named by the first `Where:` line of the prompt, fixed.
pub(crate) const FIX_OWN_FILE: &str = "prompt=$(cat)\nfile=$(printf '%s\\n' \"$prompt\" | sed -n 's/^Where: \\([^:]*\\):.*/\\1/p' | head -1)\nsed 's/bad_one/fixed_one/' \"$file\" > \"$file.new\" && mv \"$file.new\" \"$file\"\n";

/// `models` and `fix` tables of a project that lists the decision, with
/// `fix` among the lines of the `[fix]` table.
pub(crate) fn listed(fix: &str, approval: &str) -> String {
    format!("{approval}[fix]\ndecisions = [\"{BAN}\"]\n{fix}")
}

/// A project that binds `edit` to `agent`, with `rest` after the `models`
/// tables; and the directory the script lives in.
pub(crate) fn with_agent(agent: &str, rest: &str) -> (TempDir, TempDir) {
    agent_project("", agent, rest)
}

/// The same, with `rules` among the entries of the `rules` table.
pub(crate) fn agent_project(rules: &str, agent: &str, rest: &str) -> (TempDir, TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    write(scripts.path(), "agent.sh", agent);
    let script = scripts.path().join("agent.sh");
    let dir = project(&format!(
        "{rules}[models]\nedit = \"agent/m\"\n[models.providers.agent]\ncommand = [\"sh\", {:?}]\n{rest}",
        script.to_str().unwrap()
    ));
    (dir, scripts)
}

pub(crate) fn fix(dir: &TempDir, arguments: Value) -> Value {
    let mut arguments = arguments;
    arguments["decision"] = json!([BAN]);
    Client::trusting(dir.path()).tool("fix", arguments).unwrap()
}

pub(crate) fn read(dir: &TempDir, name: &str) -> String {
    fs::read_to_string(dir.path().join(name)).unwrap()
}

pub(crate) const ALLOW: &str = "[approval]\nfix = \"allow\"\n";

#[test]
fn nothing_binds_and_nothing_is_listed_so_a_fix_run_is_what_it_was() {
    let dir = project("");
    write(&dir.path().join("src"), "lib.rs", "pub fn bad_one() {}\n");

    let fixed = fix(&dir, json!({}));

    for key in ["previews", "models", "usage"] {
        assert!(fixed.get(key).is_none(), "{key} in {fixed}");
    }
    assert!(fixed["applied"].as_array().unwrap().is_empty());
    assert_eq!(read(&dir, "src/lib.rs"), "pub fn bad_one() {}\n");
}

#[test]
fn an_agent_is_bound_but_the_decision_is_not_listed_so_no_agent_runs() {
    let marks = tempfile::tempdir().unwrap();
    let calls = marks.path().join("calls");
    let (dir, _agent) = with_agent(
        &format!("echo x >> {:?}\n", calls.to_str().unwrap()),
        UNSANDBOXED,
    );

    let fixed = fix(&dir, json!({}));

    assert!(!calls.exists(), "the agent ran");
    assert!(fixed.get("models").is_none(), "{fixed}");
    assert_eq!(fixed["declined"][0]["reason"], "the rule has no fix");
}

#[test]
fn a_listed_decision_without_an_agent_is_left_for_review() {
    let dir = project(&listed("", ""));

    let fixed = fix(&dir, json!({}));

    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("`[models] edit` is not bound") && reason.contains("review show"),
        "{reason}"
    );
}

#[test]
fn a_change_outside_the_symbol_of_the_finding_is_out_of_scope() {
    let agent = format!(
        "{FIX_OWN_FILE}sed 's/fine/fine2/' src/lib.rs > src/lib.rs.new && mv src/lib.rs.new src/lib.rs\n"
    );
    let (dir, _agent) = with_agent(&agent, &listed("scope = \"symbol\"\n", ALLOW));
    write(
        dir.path(),
        "src/lib.rs",
        "pub fn bad_one() {}\npub fn fine() {}\n",
    );

    let fixed = fix(&dir, json!({}));

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "pub fn bad_one() {}\npub fn fine() {}\n"
    );
    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("line 2 of `src/lib.rs`") && reason.contains("scope `symbol`"),
        "{reason}"
    );
    assert_eq!(fixed["models"][BAN]["declined"], json!({"out-of-scope": 1}));
}

#[test]
fn a_change_inside_the_symbol_is_kept_under_allow_and_previewed_under_ask() {
    let lib = "pub fn bad_one() {}\npub fn fine() {}\n";
    for (approval, applied) in [(ALLOW, true), ("", false)] {
        let (dir, _agent) = with_agent(FIX_OWN_FILE, &listed("", approval));
        write(dir.path(), "src/lib.rs", lib);

        let fixed = fix(&dir, json!({}));

        let counts = &fixed["models"][BAN];
        assert_eq!(counts["verified"], 1, "{fixed}");
        assert_eq!(counts["applied"], i32::from(applied), "{fixed}");
        assert_eq!(counts["previewed"], i32::from(!applied), "{fixed}");
        assert_eq!(
            read(&dir, "src/lib.rs") != lib,
            applied,
            "approval {approval:?}"
        );
    }
}

/// A project with a function and its caller in another module, the agent
/// renaming the function in `src/lib.rs` and, when it is told to, the call.
fn caller_project(agent: &str, fix: &str) -> (TempDir, TempDir) {
    let (dir, scripts) = with_agent(agent, &listed(fix, ALLOW));
    write(dir.path(), ".lighthouse/decisions/ban.yaml", WIDE);
    write(
        dir.path(),
        "src/lib.rs",
        "mod other;\npub fn bad_one() {}\n",
    );
    write(
        dir.path(),
        "src/other.rs",
        "pub fn caller() {\n    crate::bad_one();\n}\n",
    );
    (dir, scripts)
}

const RENAME_BOTH: &str = "sed 's/bad_one/fixed_one/' src/lib.rs > src/lib.rs.new && mv src/lib.rs.new src/lib.rs\nsed 's/bad_one/fixed_one/' src/other.rs > src/other.rs.new && mv src/other.rs.new src/other.rs\n";

#[test]
fn with_scope_files_the_function_and_its_caller_are_fixed_together() {
    let (dir, _agent) = caller_project(RENAME_BOTH, "");

    let fixed = fix(&dir, json!({}));

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "mod other;\npub fn fixed_one() {}\n"
    );
    assert!(read(&dir, "src/other.rs").contains("crate::fixed_one();"));
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
    assert_eq!(
        fixed["applied"][0]["files"],
        json!(["src/lib.rs", "src/other.rs"])
    );
}

#[test]
fn a_caller_that_breaks_rolls_the_whole_change_back() {
    let agent = format!("{RENAME_BOTH}printf 'pub fn bad_one() {{}}\\n' >> src/other.rs\n");
    let (dir, _agent) = caller_project(&agent, "");

    let fixed = fix(&dir, json!({}));

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "mod other;\npub fn bad_one() {}\n"
    );
    assert!(read(&dir, "src/other.rs").contains("crate::bad_one();"));
    assert!(fixed["applied"].as_array().unwrap().is_empty(), "{fixed}");
    assert_eq!(fixed["models"][BAN]["declined"], json!({"introduced": 1}));
}

#[test]
fn the_scope_of_the_project_caps_a_decision_that_may_go_to_files() {
    let (dir, _agent) = caller_project(RENAME_BOTH, "scope = \"symbol\"\n");

    let fixed = fix(&dir, json!({}));

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "mod other;\npub fn bad_one() {}\n"
    );
    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("src/other.rs") && reason.contains("scope `symbol`"),
        "{reason}"
    );
}

#[test]
fn a_glob_that_matches_no_decision_is_a_warning_naming_it() {
    let dir = project("[fix]\ndecisions = [\"nonesuch/*\", \"local/naming-ban\"]\n");
    write(dir.path(), "src/lib.rs", "pub fn fine() {}\n");

    let fixed = fix(&dir, json!({}));

    let messages = fixed["messages"].to_string();
    assert!(messages.contains("lists `nonesuch/*`"), "{messages}");
    assert!(!messages.contains("lists `local/naming-ban`"), "{messages}");

    // It is said where the project is loaded, so `check` says it too.
    let out = lighthouse(dir.path(), &["check", "--no-cache"]);
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("lists `nonesuch/*`"), "{text}");
}

#[test]
fn review_show_carries_the_brief_and_the_fix_of_a_finding_whose_fix_is_not_safe() {
    let (dir, _agent) = with_agent("true\n", &listed("scope = \"symbol\"\n", ""));
    write(dir.path(), "src/lib.rs", "pub fn bad_one() {}\n");
    assert!(
        lighthouse(dir.path(), &["check", "--no-cache"])
            .status
            .success()
    );
    let fingerprint = fingerprints(dir.path())[&"bad_one".to_owned()].clone();

    let show = |dir: &TempDir| {
        Client::trusting(dir.path())
            .tool("review_show", json!({"fingerprint": &fingerprint[..12]}))
            .unwrap()
    };
    let listed_show = show(&dir);
    let brief = listed_show["brief"].as_str().expect("a brief").to_owned();
    assert_eq!(
        listed_show["fix"],
        json!({
            "by": "model",
            "safety": "unsafe",
            "runs": true,
            "approval": "ask",
            "apply": format!("lighthouse check --fix --unsafe-fixes --fingerprint {}", listed_show["fingerprint"].as_str().unwrap()),
            "tool": {"name": "fix", "arguments": {
                "fingerprints": [listed_show["fingerprint"]],
                "unsafeFixes": true
            }}
        }),
        "{listed_show}"
    );

    for expected in [
        "Decision: local/naming-ban",
        "Requirement: A function MUST NOT be named like a ban word.",
        "Message: bad_one is banned",
        "Where: src/lib.rs:1:",
        "Scope: symbol",
    ] {
        assert!(brief.contains(expected), "{expected}\n{brief}");
    }

    let (unlisted, _agent) = with_agent("true\n", "");
    write(unlisted.path(), "src/lib.rs", "pub fn bad_one() {}\n");
    assert!(
        lighthouse(unlisted.path(), &["check", "--no-cache"])
            .status
            .success()
    );
    let unlisted_show = show(&unlisted);
    assert!(
        unlisted_show["brief"].is_string(),
        "a finding without a fix has a brief"
    );
    assert!(unlisted_show.get("fix").is_none(), "{unlisted_show}");
}

#[test]
fn check_fix_tells_what_became_of_the_findings_given_to_a_model() {
    let (dir, _agent) = with_agent(FIX_OWN_FILE, &listed("", ALLOW));
    write(dir.path(), "src/lib.rs", "pub fn bad_one() {}\n");

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
    assert_eq!(read(&dir, "src/lib.rs"), "pub fn fixed_one() {}\n");
}

#[test]
fn with_models_off_no_agent_is_asked_to_fix() {
    let (dir, _agent) = with_agent(FIX_OWN_FILE, &listed("", ALLOW));
    write(dir.path(), "src/lib.rs", "pub fn bad_one() {}\n");

    let out = lighthouse(
        dir.path(),
        &["check", "--fix", "--no-models", "--no-store", "--no-cache"],
    );

    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(!text.contains("model fixes"), "{text}");
    assert_eq!(read(&dir, "src/lib.rs"), "pub fn bad_one() {}\n");
}

#[test]
fn unsafe_fixes_and_a_dry_run_apply_to_fixes_by_a_model_as_to_any_other() {
    let (dir, _agent) = with_agent(FIX_OWN_FILE, &listed("", ""));
    write(dir.path(), "src/lib.rs", "pub fn bad_one() {}\n");

    let dry = fix(&dir, json!({"unsafeFixes": true, "dryRun": true}));
    assert_eq!(read(&dir, "src/lib.rs"), "pub fn bad_one() {}\n");
    assert!(
        dry["diff"]
            .as_str()
            .unwrap()
            .contains("+pub fn fixed_one() {}")
    );

    let done = fix(&dir, json!({"unsafeFixes": true}));
    assert_eq!(done["applied"].as_array().unwrap().len(), 1, "{done}");
    assert_eq!(read(&dir, "src/lib.rs"), "pub fn fixed_one() {}\n");
}

#[test]
fn a_decision_without_a_scope_leaves_the_cap_so_a_helper_may_be_added_beside_the_target() {
    let agent = format!("{FIX_OWN_FILE}printf 'pub fn helper() {{}}\\n' >> src/lib.rs\n");
    let (dir, _agent) = with_agent(&agent, &listed("", ALLOW));
    write(dir.path(), "src/lib.rs", "pub fn bad_one() {}\n");

    let fixed = fix(&dir, json!({}));

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "pub fn fixed_one() {}\npub fn helper() {}\n",
        "{fixed}"
    );
    assert_eq!(fixed["models"][BAN]["applied"], 1, "{fixed}");
}
