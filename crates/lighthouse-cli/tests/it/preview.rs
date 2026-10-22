//! What a check says of fixes: which decision is fixable, by whom and how to
//! apply it. It computes none; a fix run shows the diff.

use std::fs;

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::checks::home;

const MISORDERED: &str = "pub fn run() -> u8 {\n    1\n}\n\npub struct Store;\n";
const BANNER: &str = "// ===== Types =====\n\npub struct Store;\n";
const DOC: &str = "pub fn undocumented() {}\n";

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let plugin = lighthouse_test_support::lang_rust();
    let write = |name: &str, text: &str| {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write(
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [\"core\", \"design\", {{ id = \"lang-rust\", path = {:?} }}]\nextends = [\"design/recommended\"]\n",
            plugin.to_str().unwrap()
        )),
    );
    write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write("src/lib.rs", source);
    dir
}

fn check(dir: &TempDir, args: &[&str]) -> String {
    let out = Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(dir.path())
        .envs(home(dir.path()))
        .args(["check", "--no-store"])
        .args(args)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

fn group(report: &Value, rule: &str) -> Value {
    report["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["decision"] == rule)
        .unwrap_or_else(|| panic!("{rule} in {report}"))
        .clone()
}

fn instance(group: &Value) -> Value {
    group["files"]["src/lib.rs"][0].clone()
}

fn report(dir: &TempDir, extra: &[&str]) -> Value {
    let mut args = vec!["--format", "json"];
    args.extend(extra);
    serde_json::from_str(&check(dir, &args)).unwrap()
}

#[test]
fn a_check_marks_what_is_fixable_and_computes_no_fix() {
    let dir = project(MISORDERED);
    let report = report(&dir, &[]);
    let group = group(&report, "design/declaration-groups");
    assert!(instance(&group).as_array().unwrap().len() == 3, "{group}");
    assert_eq!(
        group["fix"],
        json!({
            "by": "cel",
            "safety": "safe",
            "runs": true,
            "apply": "lighthouse check --fix --decision design/declaration-groups",
            "tool": {"name": "fix", "arguments": {"decision": ["design/declaration-groups"]}}
        }),
        "{group}"
    );
    assert_eq!(group["check"], json!({"by": "cel", "runs": true}));
    assert!(group.get("brief").is_none(), "a safe fix needs no brief");
    assert_eq!(
        fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        MISORDERED
    );

    let full = check(&dir, &["--format", "json", "--detail", "full"]);
    let report: Value = serde_json::from_str(full.trim()).unwrap();
    let record = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["decision"] == "design/declaration-groups")
        .unwrap();
    assert!(record.get("fix").is_none(), "{record}");
    let agent = check(&dir, &["--format", "agent"]);
    assert!(!agent.contains("@@ -"), "{agent}");
}

#[test]
fn sarif_carries_no_fix() {
    let dir = project(MISORDERED);
    let sarif = check(&dir, &["--format", "sarif"]);
    assert!(sarif.contains("design/declaration-groups"), "{sarif}");
    assert!(!sarif.contains("\"fixes\""), "{sarif}");
}

#[test]
fn an_unsafe_fix_is_marked_with_whether_it_is_written_and_a_brief() {
    let dir = project(BANNER);
    let compact = report(&dir, &[]);
    let group = group(&compact, "design/comments");
    assert_eq!(group["fix"]["safety"], "unsafe", "{group}");
    assert_eq!(group["fix"]["written"], false, "{group}");
    assert!(
        group["fix"]["tool"]["arguments"]
            .get("unsafeFixes")
            .is_none()
    );
    assert!(
        group["fix"]["apply"]
            .as_str()
            .unwrap()
            .starts_with("lighthouse check --fix --decision ")
    );
    assert_eq!(group["brief"], true, "an unsafe fix has a brief");
    assert_eq!(instance(&group).as_array().unwrap().len(), 3, "{group}");
}

#[test]
fn a_finding_without_a_fix_has_a_brief() {
    let dir = project(DOC);
    let plain = report(&dir, &[]);
    let undocumented = group(&plain, "design/exported-doc");
    assert_eq!(instance(&undocumented).as_array().unwrap().len(), 3);
    assert!(undocumented.get("fix").is_none());
    assert_eq!(
        undocumented["brief"], true,
        "no fix: review_show has a brief"
    );
}

const SHOUT: &str = "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/shout\nspec:\n  title: Notes are upper case\n  context: Notes are read from far away.\n  scope:\n    subject: file\n  requirement: A note MUST be written in upper case.\n  severity: error\n  check:\n    type: cel\n    select: file\n    where: \"file.path.endsWith(\\\".txt\\\") && file.lines > 0\"\n    message: \"{{ file.path }} is not shouting\"\n    evidence:\n      path: file.path\n  fix:\n    safety: safe\n    type: command\n    command: [sh, \"-c\", \"tr a-z A-Z < \\\"$1\\\"\", sh, \"{file}\"]\n    output: stdout\n  examples:\n    - name: quiet\n      language: text\n      kind: invalid\n      files:\n        - path: a.txt\n          body: hello\n      expect:\n        - line: 1\n      fixed:\n        - path: a.txt\n          body: HELLO\n    - name: loud\n      language: text\n      kind: valid\n      files:\n        - path: a.txt\n          body: HELLO\n";

#[test]
fn a_command_fix_says_why_it_does_not_run_untrusted() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, text: &str| {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write(
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\", \"local\"]\n[rules]\n\"local/shout\" = \"error\"\n",
        ),
    );
    write(".lighthouse/decisions/shout.yaml", SHOUT);
    write("a.txt", "hello\n");
    let out = Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join(".home"))
        .env("XDG_DATA_HOME", dir.path().join(".home"))
        .env("XDG_CACHE_HOME", dir.path().join(".home"))
        .env_remove("LIGHTHOUSE_TRUST")
        .args(["check", "--no-store", "--format", "json"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let shout = group(&report, "local/shout");
    assert_eq!(
        shout["fix"],
        json!({
            "by": "command",
            "safety": "safe",
            "runs": false,
            "why": "untrusted project",
            "apply": "lighthouse check --fix --decision local/shout",
            "tool": {"name": "fix", "arguments": {"decision": ["local/shout"]}}
        }),
        "{shout}"
    );
    assert_eq!(shout["brief"], true, "a fix that does not run has a brief");
    assert_eq!(
        shout["files"]["a.txt"][0].as_array().unwrap().len(),
        3,
        "{shout}"
    );
}
