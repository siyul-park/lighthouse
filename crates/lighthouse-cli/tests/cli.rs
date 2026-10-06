use std::{fs, path::Path};

use assert_cmd::Command;
use tempfile::TempDir;

fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir);
    cmd
}

fn project(max: usize) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        format!("plugins = [\"core\"]\n[rules]\n\"core/max-file-lines\" = {{ level = \"warn\", max = {max} }}\n"),
    )
    .unwrap();
    fs::write(dir.path().join("big.txt"), "a\nb\nc\nd\ne\nf\ng\nh\n").unwrap();
    fs::write(dir.path().join("small.txt"), "a\n").unwrap();
    dir
}

#[test]
fn check_reports_warning_and_passes_without_strict() {
    let dir = project(6);
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .success()
        .stdout("big.txt:7:1: warn core/max-file-lines: file has 8 lines, limit is 6\n");
}

#[test]
fn strict_fails_on_warnings() {
    let dir = project(6);
    lighthouse(dir.path())
        .args(["check", "--strict"])
        .assert()
        .code(1);
}

#[test]
fn clean_project_exits_zero_with_no_output() {
    let dir = project(10);
    lighthouse(dir.path())
        .args(["check", "--strict"])
        .assert()
        .success()
        .stdout("");
}

#[test]
fn json_and_sarif_formats() {
    let dir = project(6);
    let out = lighthouse(dir.path())
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    let line: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(line["rule_id"], "core/max-file-lines");
    assert_eq!(line["evidence"]["lines"], 8);

    let out = lighthouse(dir.path())
        .args(["check", "--format", "sarif"])
        .output()
        .unwrap();
    let sarif: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    assert!(sarif["runs"][0]["results"][0]["partialFingerprints"]["lighthouse/v1"].is_string());
}

#[test]
fn gitignored_files_are_skipped_and_rules_filter_applies() {
    let dir = project(6);
    fs::write(dir.path().join(".gitignore"), "big.txt\n").unwrap();
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .success()
        .stdout("");

    let dir = project(6);
    lighthouse(dir.path())
        .args(["check", "--rules", "core/nope"])
        .assert()
        .code(2)
        .stderr("lighthouse: unknown rule `core/nope`\n");
}

#[test]
fn override_can_turn_a_rule_off_for_a_file() {
    let dir = project(6);
    let config = dir.path().join("lighthouse.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(
        "[[overrides]]\nfiles = [\"big.txt\"]\nrules = { \"core/max-file-lines\" = \"off\" }\n",
    );
    fs::write(&config, text).unwrap();
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .success()
        .stdout("");
}

#[test]
fn missing_config_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(2)
        .stderr("lighthouse: no lighthouse.toml found (run `lighthouse init`)\n");
}

#[test]
fn init_writes_config_that_checks_clean_and_refuses_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    lighthouse(dir.path()).arg("init").assert().success();
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .success()
        .stdout("");
    lighthouse(dir.path()).arg("init").assert().code(2);
}

#[test]
fn rule_list_and_explain() {
    let dir = tempfile::tempdir().unwrap();
    lighthouse(dir.path())
        .args(["rule", "list"])
        .assert()
        .success()
        .stdout(
            "core/max-file-lines\twarn\tFiles stay below a line limit
design/complexity-signal\twarn\tComplexity is a review signal
design/coupling-signal\twarn\tCoupling is a review signal
design/exported-doc\twarn\tExported symbols are documented
design/single-use-wrapper\twarn\tInline single-use wrappers
",
        );
    let out = lighthouse(dir.path())
        .args(["explain", "core/max-file-lines"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("Analyzers: core/line-count")
    );
    lighthouse(dir.path())
        .args(["explain", "core/nope"])
        .assert()
        .code(2);
}

#[test]
fn rule_list_all_shows_status_of_every_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let out = lighthouse(dir.path())
        .args(["rule", "list", "--all"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    for line in [
        "core/max-file-lines\timplemented\twarn\tFiles stay below a line limit",
        "design/error-identity\tunimplemented\twarn\tPreserve error identity",
        "design/signals-are-advisory\tdoc\t-\tSignals stay advisory",
    ] {
        assert!(text.lines().any(|l| l == line), "{line}");
    }
}

#[test]
fn explain_describes_unimplemented_patterns() {
    let dir = tempfile::tempdir().unwrap();
    let out = lighthouse(dir.path())
        .args(["explain", "design/error-identity"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("**Requirement**\n\nDependency identity MUST be preserved"));
    assert!(text.contains("%w"));
    assert!(text.contains("Status: unimplemented"));
}

#[test]
fn docs_check_fails_until_generated_and_after_edits() {
    let dir = tempfile::tempdir().unwrap();
    lighthouse(dir.path())
        .args(["docs", "check"])
        .assert()
        .code(1);
    lighthouse(dir.path())
        .args(["docs", "generate"])
        .assert()
        .success();
    lighthouse(dir.path())
        .args(["docs", "check"])
        .assert()
        .success();

    let page = dir.path().join("docs/patterns/design.md");
    fs::write(&page, "edited\n").unwrap();
    lighthouse(dir.path())
        .args(["docs", "check"])
        .assert()
        .code(1);
    lighthouse(dir.path())
        .args(["docs", "generate", "--out", "other"])
        .assert()
        .success();
    assert!(dir.path().join("other/patterns/testing.md").exists());
}

#[test]
fn docs_generate_removes_orphans_and_check_flags_them() {
    let dir = tempfile::tempdir().unwrap();
    lighthouse(dir.path())
        .args(["docs", "generate"])
        .assert()
        .success();
    let orphan = dir.path().join("docs/patterns/old.md");
    fs::write(&orphan, "x").unwrap();
    let out = lighthouse(dir.path())
        .args(["docs", "check"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("old.md is not generated")
    );
    lighthouse(dir.path())
        .args(["docs", "generate"])
        .assert()
        .success();
    assert!(!orphan.exists());
    lighthouse(dir.path())
        .args(["docs", "check"])
        .assert()
        .success();
}

#[test]
fn docs_check_reports_unreadable_output_as_an_error() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("docs/patterns/design.md")).unwrap();
    lighthouse(dir.path())
        .args(["docs", "check"])
        .assert()
        .code(2);
}

#[test]
fn config_flag_checks_the_current_directory_with_another_config_file() {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("go.mod"), "module example.com/app\n").unwrap();
    fs::write(
        project.path().join("api.go"),
        "package app\n\nfunc Open() {}\n",
    )
    .unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let config = elsewhere.path().join("lh.toml");
    fs::write(
        &config,
        "plugins = [\"lang-go\", \"design\"]\nextends = [\"design/recommended\"]\n",
    )
    .unwrap();
    lighthouse(project.path())
        .args(["check", "--config"])
        .arg(&config)
        .assert()
        .success()
        .stdout("api.go:3:1: warn design/exported-doc: exported function Open must have a doc comment\n");
    lighthouse(project.path())
        .args(["check", "--config", "missing.toml"])
        .assert()
        .code(2);
}
