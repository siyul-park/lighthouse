//! Who may have a finding fixed by a model, and how far the change may go:
//! `[fix] decisions`, the scope of the fix, the chain of runs of a pass and
//! the brief of a review task. The agent is a small shell script that edits
//! the files of its working directory, so no real agent is run.

use std::fs;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{
    cache_fixtures::write,
    mcp::Client,
    models::project,
    models_plugin::{fingerprints, lighthouse},
};

const BAN: &str = "local/naming-ban";

/// The decision of `models.rs` with the fix block of a model that may go to
/// any file.
const WIDE: &str = r#"apiVersion: lighthouse/v1alpha1
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
    safety: suggested
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
const FIX_OWN_FILE: &str = "prompt=$(cat)\nfile=$(printf '%s\\n' \"$prompt\" | sed -n 's/^Where: \\([^:]*\\):.*/\\1/p' | head -1)\nsed 's/bad_one/fixed_one/' \"$file\" > \"$file.new\" && mv \"$file.new\" \"$file\"\n";

/// `models` and `fix` tables of a project that lists the decision, with
/// `fix` among the lines of the `[fix]` table.
fn listed(fix: &str, approval: &str) -> String {
    format!("{approval}[fix]\ndecisions = [\"{BAN}\"]\n{fix}")
}

/// A project that binds `edit` to `agent`, with `rest` after the `models`
/// tables; and the directory the script lives in.
fn with_agent(agent: &str, rest: &str) -> (TempDir, TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    write(scripts.path(), "agent.sh", agent);
    let script = scripts.path().join("agent.sh");
    let dir = project(&format!(
        "[models]\nedit = \"agent/m\"\n[models.providers.agent]\ncommand = [\"sh\", {:?}]\n{rest}",
        script.to_str().unwrap()
    ));
    (dir, scripts)
}

fn fix(dir: &TempDir, arguments: Value) -> Value {
    let mut arguments = arguments;
    arguments["decision"] = json!([BAN]);
    Client::trusting(dir.path()).tool("fix", arguments).unwrap()
}

fn read(dir: &TempDir, name: &str) -> String {
    fs::read_to_string(dir.path().join(name)).unwrap()
}

const ALLOW: &str = "[approval]\nfix = \"allow\"\n";

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
    let (dir, _agent) = with_agent(&format!("echo x >> {:?}\n", calls.to_str().unwrap()), "");

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
    let (dir, _agent) = with_agent(&agent, &listed("", ALLOW));
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

/// Two modules with a banned function each, and a shared file.
fn chain_project(agent: &str, rest: &str) -> (TempDir, TempDir) {
    let (dir, scripts) = with_agent(agent, rest);
    write(dir.path(), ".lighthouse/decisions/ban.yaml", WIDE);
    write(dir.path(), "src/lib.rs", "mod a;\nmod b;\nmod shared;\n");
    write(dir.path(), "src/a.rs", "pub fn bad_one() {}\n");
    write(dir.path(), "src/b.rs", "pub fn bad_one() {}\n");
    write(dir.path(), "src/shared.rs", "// log\n");
    (dir, scripts)
}

#[test]
fn a_later_run_starts_from_what_the_earlier_ones_were_accepted_with() {
    let agent = format!("{FIX_OWN_FILE}echo '// run' >> src/shared.rs\n");
    let (dir, _agent) = chain_project(&agent, &listed("", ALLOW));

    let fixed = fix(&dir, json!({}));

    assert_eq!(read(&dir, "src/a.rs"), "pub fn fixed_one() {}\n");
    assert_eq!(read(&dir, "src/b.rs"), "pub fn fixed_one() {}\n");
    assert_eq!(
        read(&dir, "src/shared.rs"),
        "// log\n// run\n// run\n",
        "the second run saw what the first wrote"
    );
    assert_eq!(fixed["usage"]["edit"]["runs"], 2, "{fixed}");
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 2, "{fixed}");
}

#[test]
fn a_failing_verify_rolls_the_newest_run_back_and_keeps_the_ones_before() {
    let verify = "verify = [[\"sh\", \"-c\", \"! grep -q fixed_one src/b.rs\"]]\n";
    let (dir, _agent) = chain_project(FIX_OWN_FILE, &listed(verify, ALLOW));

    let fixed = fix(&dir, json!({}));

    assert_eq!(read(&dir, "src/a.rs"), "pub fn fixed_one() {}\n");
    assert_eq!(read(&dir, "src/b.rs"), "pub fn bad_one() {}\n");
    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
    let reason = fixed["declined"][0]["reason"].as_str().unwrap();
    assert!(reason.starts_with("rolled back"), "{reason}");
    assert_eq!(fixed["models"][BAN]["declined"], json!({"rolled-back": 1}));
}

#[test]
fn a_glob_that_matches_no_decision_is_a_warning_naming_it() {
    let dir = project("[fix]\ndecisions = [\"nonesuch/*\", \"local/naming-ban\"]\n");
    write(dir.path(), "src/lib.rs", "pub fn fine() {}\n");

    let fixed = fix(&dir, json!({}));

    let messages = fixed["messages"].to_string();
    assert!(messages.contains("lists `nonesuch/*`"), "{messages}");
    assert!(!messages.contains("lists `local/naming-ban`"), "{messages}");
}

#[test]
fn review_show_carries_the_brief_of_a_listed_decision_only() {
    let (dir, _agent) = with_agent("true\n", &listed("scope = \"symbol\"\n", ""));
    write(dir.path(), "src/lib.rs", "pub fn bad_one() {}\n");
    assert!(
        lighthouse(dir.path(), &["check", "--no-cache"])
            .status
            .success()
    );
    let fingerprint = fingerprints(dir.path())[&"bad_one".to_owned()].clone();

    let show = |dir: &TempDir| {
        let shown = Client::trusting(dir.path())
            .tool("review_show", json!({"fingerprint": &fingerprint[..12]}))
            .unwrap();
        shown["brief"].as_str().map(str::to_owned)
    };
    let brief = show(&dir).expect("a brief");

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
    assert_eq!(show(&unlisted), None);
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

/// An error decision with a safe fix of its own: a helper named `zzz_helper`
/// is renamed, the way a reorder puts code where a rule wants it.
const TIDY: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/helper-name
spec:
  title: Helper name
  context: A helper has the name the project gives it.
  scope: { subject: symbol }
  requirement: A helper MUST NOT be named zzz_helper.
  severity: error
  check:
    type: cel
    where: symbol.kind == "function" && symbol.name == "zzz_helper"
    message: "{{ symbol.name }} is misnamed"
  fix:
    type: ops
    safety: safe
    ops:
      - op: replace
        file: finding.file
        span: finding.span
        text: pub fn aaa_helper() {}
  examples:
    - name: bad
      language: rust
      kind: invalid
      files: [{ path: src/lib.rs, body: "pub fn zzz_helper() {}\n" }]
      fixed: [{ path: src/lib.rs, body: "pub fn aaa_helper() {}\n" }]
      expect: [{ line: 1 }]
    - name: good
      language: rust
      kind: valid
      files: [{ path: src/lib.rs, body: "pub fn aaa_helper() {}\n" }]
"#;

/// A project whose agent fixes the banned function and adds a misnamed
/// helper, which the safe fix of `local/helper-name` renames; and the file
/// the agent keeps its brief in.
fn settled_project(approval: &str) -> (TempDir, TempDir, std::path::PathBuf) {
    let scripts = tempfile::tempdir().unwrap();
    let told = scripts.path().join("told");
    let keep = format!(
        "prompt=$(cat)\nprintf '%s\\n' \"$prompt\" > {:?}\n",
        told.to_str().unwrap()
    );
    let agent = format!(
        "{}printf 'pub fn zzz_helper() {{}}\\n' >> src/lib.rs\n",
        FIX_OWN_FILE.replacen("prompt=$(cat)\n", &keep, 1)
    );
    write(scripts.path(), "agent.sh", &agent);
    let script = scripts.path().join("agent.sh");
    let dir = project(&format!(
        "\"local/helper-name\" = \"error\"\n[models]\nedit = \"agent/m\"\n[models.providers.agent]\ncommand = [\"sh\", {:?}]\n{}",
        script.to_str().unwrap(),
        listed("", approval)
    ));
    write(dir.path(), ".lighthouse/decisions/ban.yaml", WIDE);
    write(dir.path(), ".lighthouse/decisions/tidy.yaml", TIDY);
    write(dir.path(), "src/lib.rs", "pub fn bad_one() {}\n");
    (dir, scripts, told)
}

#[test]
fn the_safe_fixes_of_the_project_run_on_an_edit_before_it_is_judged() {
    let (dir, _agent, told) = settled_project(ALLOW);

    let fixed = fix(&dir, json!({}));

    assert_eq!(
        read(&dir, "src/lib.rs"),
        "pub fn fixed_one() {}\npub fn aaa_helper() {}\n",
        "{fixed}"
    );
    assert_eq!(fixed["models"][BAN]["verified"], 1, "{fixed}");
    assert_eq!(fixed["models"][BAN]["applied"], 1, "{fixed}");
    let brief = fs::read_to_string(&told).unwrap();
    assert!(
        brief.contains("The project's tools fix these after you")
            && brief.contains("local/helper-name"),
        "{brief}"
    );
}

#[test]
fn the_preview_of_an_edit_shows_the_text_the_safe_fixes_left() {
    let (dir, _agent, _told) = settled_project("");

    let fixed = fix(&dir, json!({}));

    assert_eq!(read(&dir, "src/lib.rs"), "pub fn bad_one() {}\n");
    let preview = fixed["previews"][0].to_string();
    assert!(preview.contains("aaa_helper"), "{preview}");
    assert!(!preview.contains("zzz_helper"), "{preview}");
}
