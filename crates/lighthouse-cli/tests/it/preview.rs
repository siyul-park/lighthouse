//! Fix previews in reports: the agent formats show the fix of a finding as a
//! unified diff and SARIF as `fixes`, both computed without writing anything.

use std::fs;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

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
        .find(|g| g["rule"] == rule)
        .unwrap_or_else(|| panic!("{rule} in {report}"))
        .clone()
}

fn instance(group: &Value) -> Value {
    group["files"]["src/lib.rs"][0].clone()
}

fn report(dir: &TempDir, extra: &[&str]) -> Value {
    let mut args = vec!["--format", "agent-json"];
    args.extend(extra);
    serde_json::from_str(&check(dir, &args)).unwrap()
}

#[test]
fn a_declaration_move_shows_its_diff_and_how_to_apply_it() {
    let dir = project(MISORDERED);
    let report = report(&dir, &[]);
    let group = group(&report, "design/declaration-groups");
    let fix = &instance(&group)[3]["fix"];
    assert_eq!(fix["safety"], "safe", "{group}");
    let diff = fix["diff"].as_str().unwrap();
    assert!(diff.starts_with("@@ -"), "{diff}");
    assert!(diff.contains("+pub struct Store;"), "{diff}");
    assert!(diff.lines().count() <= 12, "{diff}");
    assert_eq!(
        group["apply"],
        "lighthouse check --fix --rules design/declaration-groups"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        MISORDERED
    );

    let text = check(&dir, &["--format", "agent"]);
    assert!(text.contains("    fix [safe]\n      @@ -"), "{text}");
    assert!(text.contains("      +pub struct Store;"), "{text}");

    let full = check(&dir, &["--format", "agent-json", "--detail", "full"]);
    let record = full
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|r| r["rule"] == "design/declaration-groups")
        .unwrap();
    assert!(record["fix"]["diff"].as_str().unwrap().starts_with("@@ -"));
}

#[test]
fn a_long_fix_is_summarized_and_a_finding_without_a_fix_has_none() {
    let funcs: String = (0..6)
        .map(|i| format!("pub fn run{i}() -> u8 {{\n    {i}\n}}\n\n"))
        .collect();
    let types: String = (0..6).map(|i| format!("pub struct Store{i};\n")).collect();
    let source = format!("{funcs}{types}");
    let dir = project(&source);
    let long = report(&dir, &[]);
    let moved = group(&long, "design/declaration-groups");
    let fix = &instance(&moved)[3]["fix"];
    assert_eq!(fix["safety"], "safe");
    assert!(fix.get("diff").is_none(), "{fix}");
    assert!(fix["summary"].as_str().unwrap().contains("lines)"), "{fix}");

    let dir = project(DOC);
    let plain = report(&dir, &[]);
    let undocumented = group(&plain, "design/exported-doc");
    assert_eq!(instance(&undocumented).as_array().unwrap().len(), 3);
    assert!(undocumented.get("apply").is_none());
}

#[test]
fn sarif_carries_the_fix_as_replacements_of_regions() {
    let dir = project(MISORDERED);
    let sarif: Value = serde_json::from_str(&check(&dir, &["--format", "sarif"])).unwrap();
    let results = sarif["runs"][0]["results"].as_array().unwrap();
    let moved = results
        .iter()
        .find(|r| r["ruleId"] == "design/declaration-groups")
        .unwrap();
    let fix = &moved["fixes"][0];
    assert!(fix["description"]["text"].is_string(), "{fix}");
    let change = &fix["artifactChanges"][0];
    assert_eq!(change["artifactLocation"]["uri"], "src/lib.rs");
    let replacements = change["replacements"].as_array().unwrap();
    assert!(replacements.len() >= 2, "{change}");
    assert!(
        replacements
            .iter()
            .all(|r| r["deletedRegion"]["startLine"].is_u64())
    );
    assert!(replacements.iter().any(|r| {
        r["insertedContent"]["text"]
            .as_str()
            .unwrap()
            .contains("pub struct Store;")
    }));
    let documented = results
        .iter()
        .find(|r| r["ruleId"] == "design/exported-doc");
    assert!(documented.is_none_or(|r| r.get("fixes").is_none()));
}

#[test]
fn a_delete_fix_replaces_a_region_with_nothing() {
    let dir = project(BANNER);
    let sarif: Value = serde_json::from_str(&check(&dir, &["--format", "sarif"])).unwrap();
    let banner = sarif["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["ruleId"] == "design/section-banners")
        .unwrap_or_else(|| panic!("{sarif}"));
    let replacement = &banner["fixes"][0]["artifactChanges"][0]["replacements"][0];
    assert_eq!(replacement["insertedContent"]["text"], "");
    assert_eq!(replacement["deletedRegion"]["startLine"], 1);

    let compact = report(&dir, &[]);
    let group = group(&compact, "design/section-banners");
    assert_eq!(instance(&group)[3]["fix"]["safety"], "suggested", "{group}");
    assert!(group["apply"].as_str().unwrap().contains("--unsafe-fixes"));
    assert!(
        instance(&group)[3]["fix"]["diff"]
            .as_str()
            .unwrap()
            .contains("-// ===== Types =====")
    );
}

const SHOUT: &str = "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/shout\nspec:\n  title: Notes are upper case\n  context: Notes are read from far away.\n  scope:\n    domain: code\n    subject: file\n  requirement: A note MUST be written in upper case.\n  severity: error\n  check:\n    type: cel\n    select: file\n    where: \"file.path.endsWith(\\\".txt\\\") && file.lines > 0\"\n    message: \"{{ file.path }} is not shouting\"\n    evidence:\n      path: file.path\n  fix:\n    safety: safe\n    type: command\n    argv: [sh, \"-c\", \"tr a-z A-Z < \\\"$1\\\"\", sh, \"{file}\"]\n    output: text\n  examples:\n    - name: quiet\n      language: text\n      kind: invalid\n      files:\n        - path: a.txt\n          body: hello\n      expect:\n        - line: 1\n      fixed:\n        - path: a.txt\n          body: HELLO\n    - name: loud\n      language: text\n      kind: valid\n      files:\n        - path: a.txt\n          body: HELLO\n";

#[test]
fn a_command_fix_is_not_previewed_and_only_names_how_to_apply_it() {
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
        .env("LIGHTHOUSE_HOME", dir.path().join(".home"))
        .args(["check", "--no-store", "--format", "agent-json"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let shout = group(&report, "local/shout");
    assert_eq!(shout["apply"], "lighthouse check --fix --rules local/shout");
    assert_eq!(
        shout["files"]["a.txt"][0].as_array().unwrap().len(),
        3,
        "{shout}"
    );
}
