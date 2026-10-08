use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;
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
        lighthouse_testkit::project(&format!("plugins = [\"core\"]\n[rules]\n\"core/max-file-lines\" = {{ level = \"warn\", options = {{ max = {max} }} }}\n")),
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
        .stdout("big.txt:7:1: warn core/max-file-lines: file has 8 lines, limit is 6\nsummary: 0 error, 1 warn, 0 info, 0 incomplete\n");
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
    assert_eq!(line["ruleId"], "core/max-file-lines");
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
    fs::write(
        &config,
        lighthouse_testkit::project(
            "plugins = [\"core\"]\n[rules]\n\"core/max-file-lines\" = { level = \"warn\", options = { max = 6 } }\n[[overrides]]\nfiles = [\"big.txt\"]\nrules = { \"core/max-file-lines\" = \"off\" }\n",
        ),
    )
    .unwrap();
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
fn init_keeps_the_store_out_of_version_control() {
    let fresh = tempfile::tempdir().unwrap();
    lighthouse(fresh.path()).arg("init").assert().success();
    let ignore = |dir: &TempDir| fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert_eq!(ignore(&fresh), ".lighthouse/*.db*\n");

    let existing = tempfile::tempdir().unwrap();
    fs::write(existing.path().join(".gitignore"), "target").unwrap();
    lighthouse(existing.path()).arg("init").assert().success();
    assert_eq!(ignore(&existing), "target\n.lighthouse/*.db*\n");

    let attributes = fs::read_to_string(fresh.path().join(".gitattributes")).unwrap();
    assert_eq!(attributes, ".lighthouse/decisions.jsonl merge=union\n");

    let ignoring = tempfile::tempdir().unwrap();
    fs::write(ignoring.path().join(".gitignore"), ".lighthouse/*.db*\n").unwrap();
    lighthouse(ignoring.path()).arg("init").assert().success();
    assert_eq!(ignore(&ignoring), ".lighthouse/*.db*\n");
}

#[test]
fn decision_list_and_explain() {
    let dir = tempfile::tempdir().unwrap();
    let list = lighthouse(dir.path())
        .args(["decision", "list"])
        .output()
        .unwrap();
    let list = String::from_utf8(list.stdout).unwrap();
    for line in [
        "core/max-file-lines\twarn\tFiles stay below a line limit",
        "design/complexity-signal\twarn\tComplexity is a review signal",
        "design/declaration-groups\terror\t",
        "design/no-exported-mutable-global\twarn\t",
        "design/private-helper-callers\tinfo\t",
        "testing/owner-test\twarn\t",
        "testing/single-owner-test\terror\t",
    ] {
        assert!(
            list.lines().any(|l| l.starts_with(line)),
            "{line} not in {list}"
        );
    }
    let out = lighthouse(dir.path())
        .args(["explain", "design/coupling-signal"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("Analyzers: metrics/size")
    );
    lighthouse(dir.path())
        .args(["explain", "core/nope"])
        .assert()
        .code(2);
}

#[test]
fn decision_list_all_shows_status_of_every_decision() {
    let dir = tempfile::tempdir().unwrap();
    let out = lighthouse(dir.path())
        .args(["decision", "list", "--all"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    for line in [
        "core/max-file-lines\timplemented\twarn\tFiles stay below a line limit",
        "design/error-identity\tjudged\twarn\tPreserve error identity",
        "design/signals-are-advisory\tdoc\t-\tSignals stay advisory",
    ] {
        assert!(text.lines().any(|l| l == line), "{line}");
    }
}

#[test]
fn explain_describes_judged_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let out = lighthouse(dir.path())
        .args(["explain", "design/error-identity"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("**Requirement**\n\nDependency identity MUST be preserved"));
    assert!(text.contains("%w"));
    assert!(text.contains("Status: judged"));
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

    let page = dir.path().join("docs/decisions/design.md");
    fs::write(&page, "edited\n").unwrap();
    lighthouse(dir.path())
        .args(["docs", "check"])
        .assert()
        .code(1);
    lighthouse(dir.path())
        .args(["docs", "generate", "--out", "other"])
        .assert()
        .success();
    assert!(dir.path().join("other/decisions/testing.md").exists());
}

#[test]
fn docs_generate_removes_orphans_and_check_flags_them() {
    let dir = tempfile::tempdir().unwrap();
    lighthouse(dir.path())
        .args(["docs", "generate"])
        .assert()
        .success();
    let orphan = dir.path().join("docs/decisions/old.md");
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
    fs::create_dir_all(dir.path().join("docs/decisions/design.md")).unwrap();
    lighthouse(dir.path())
        .args(["docs", "check"])
        .assert()
        .code(2);
}

/// A Go project and a config listing the Go plugin built from source; `None`
/// after a skip message when no Go toolchain is available.
fn go_project(source: &str) -> Option<(TempDir, std::path::PathBuf)> {
    let plugin = lighthouse_testkit::lang_go()?;
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("go.mod"), "module example.com/app\n").unwrap();
    fs::write(project.path().join("api.go"), source).unwrap();
    let config = project.path().join("lighthouse.toml");
    fs::write(
        &config,
        lighthouse_testkit::project(&format!(
            "plugins = [{{ id = \"lang-go\", path = {:?} }}, \"design\"]\nextends = [\"design/recommended\"]\n",
            plugin.to_str().unwrap()
        )),
    )
    .unwrap();
    Some((project, config))
}

#[test]
fn go_files_are_analyzed_by_the_go_plugin_over_rpc() {
    let Some((project, _)) = go_project("package app\n\nfunc Open() {}\n") else {
        return;
    };
    lighthouse(project.path())
        .arg("check")
        .assert()
        .success()
        .stdout("api.go:3:1: warn design/exported-doc: exported function Open must have a doc comment\nsummary: 0 error, 1 warn, 0 info, 0 incomplete\n");
}

#[test]
fn incomplete_analysis_exits_3_and_says_so_in_every_format() {
    let Some((project, _)) = go_project("package app\n\nfunc Open( {\n") else {
        return;
    };
    let out = lighthouse(project.path()).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("api.go: incomplete: "), "{stdout}");

    let out = lighthouse(project.path())
        .args(["check", "--format", "sarif"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let sarif: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["executionSuccessful"],
        false
    );

    let out = lighthouse(project.path())
        .args(["check", "--allow-incomplete"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("incomplete")
    );
}

#[test]
fn reporting_paths_do_not_narrow_the_analysis_of_incomplete_files() {
    let Some((project, _)) = go_project("package app\n\nfunc Open( {\n") else {
        return;
    };
    fs::create_dir(project.path().join("sub")).unwrap();
    fs::write(project.path().join("sub/ok.go"), "package sub\n").unwrap();
    lighthouse(project.path())
        .args(["check", "sub"])
        .assert()
        .code(3);
}

#[test]
fn a_plugin_listed_but_missing_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_testkit::project("plugins = [\"nowhere\"]\n"),
    )
    .unwrap();
    lighthouse(dir.path()).arg("check").assert().code(2);
}

#[test]
fn config_flag_checks_the_current_directory_with_another_config_file() {
    let Some((project, _)) = go_project("package app\n\nfunc Open() {}\n") else {
        return;
    };
    let plugin = lighthouse_testkit::lang_go().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let config = elsewhere.path().join("lh.toml");
    fs::write(
        &config,
        lighthouse_testkit::project(&format!(
            "plugins = [{{ id = \"lang-go\", path = {:?} }}, \"design\"]\nextends = [\"design/recommended\"]\n",
            plugin.to_str().unwrap()
        )),
    )
    .unwrap();
    fs::remove_file(project.path().join("lighthouse.toml")).unwrap();
    lighthouse(project.path())
        .args(["check", "--config"])
        .arg(&config)
        .assert()
        .success()
        .stdout("api.go:3:1: warn design/exported-doc: exported function Open must have a doc comment\nsummary: 0 error, 1 warn, 0 info, 0 incomplete\n");
    lighthouse(project.path())
        .args(["check", "--config", "missing.toml"])
        .assert()
        .code(2);
}

fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn repository() -> TempDir {
    let dir = project(6);
    fs::write(dir.path().join(".gitignore"), "target\n").unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "base"]);
    dir
}

#[test]
fn changed_reports_only_modified_and_untracked_files_but_analyzes_everything() {
    let dir = repository();
    lighthouse(dir.path())
        .args(["check", "--changed"])
        .assert()
        .success()
        .stdout("");
    fs::write(dir.path().join("new.txt"), "a\nb\nc\nd\ne\nf\ng\nh\n").unwrap();
    lighthouse(dir.path())
        .args(["check", "--changed"])
        .assert()
        .success()
        .stdout(predicates_contains("new.txt:7:1: warn core/max-file-lines"))
        .stdout(predicates_lacks("big.txt"));
    fs::write(dir.path().join("big.txt"), "a\nb\nc\nd\ne\nf\ng\nh\ni\n").unwrap();
    let out = lighthouse(dir.path())
        .args(["check", "--changed"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("big.txt:7:1") && text.contains("new.txt:7:1"),
        "{text}"
    );
}

#[test]
fn diff_reports_files_changed_since_the_merge_base() {
    let dir = repository();
    git(dir.path(), &["checkout", "-q", "-b", "work"]);
    fs::write(dir.path().join("small.txt"), "a\nb\nc\nd\ne\nf\ng\nh\n").unwrap();
    git(dir.path(), &["commit", "-q", "-am", "grow"]);
    let out = lighthouse(dir.path())
        .args(["check", "--diff", "main"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("small.txt:7:1") && !text.contains("big.txt"),
        "{text}"
    );
    lighthouse(dir.path())
        .args(["check", "--diff", "no-such-branch"])
        .assert()
        .code(2);
}

#[test]
fn changed_and_diff_exclude_each_other() {
    let dir = repository();
    lighthouse(dir.path())
        .args(["check", "--changed", "--diff", "main"])
        .assert()
        .code(2);
}

fn predicates_contains(text: &'static str) -> impl predicates::Predicate<[u8]> {
    predicates::str::contains(text).from_utf8()
}

fn predicates_lacks(text: &'static str) -> impl predicates::Predicate<[u8]> {
    predicates::str::contains(text).not().from_utf8()
}

const LOCAL_RULE: &str = "apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/long-file
spec:
  title: Files stay short
  intent: Long files are hard to read.
  scope: { subject: file }
  requirement: A file MUST have at most three lines.
  severity: error
  evidence: [path]
  check:
    type: cel
    select: file
    where: 'file.lines > 3 && !file.generated'
    message: 'file {{ file.path }} has {{ file.lines }} lines'
    evidence:
      path: file.path
  examples:
    - name: long
      language: text
      kind: invalid
      files: [{ path: a.txt, body: \"1\\n2\\n3\\n4\" }]
      expect: [{ line: 1 }]
    - name: short
      language: text
      kind: valid
      files: [{ path: a.txt, body: \"1\\n2\" }]
";

fn local_project(rule: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_testkit::project(
            "plugins = [\"core\", \"local\"]\n[rules]\n\"local/long-file\" = \"error\"\n",
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.path().join(".lighthouse/decisions")).unwrap();
    fs::write(
        dir.path().join(".lighthouse/decisions/long-file.yaml"),
        rule,
    )
    .unwrap();
    fs::write(dir.path().join("big.txt"), "1\n2\n3\n4\n5\n").unwrap();
    fs::write(dir.path().join("small.txt"), "1\n").unwrap();
    dir
}

#[test]
fn local_declarative_rules_run_in_check_and_are_tested_by_decision_test() {
    let dir = local_project(LOCAL_RULE);
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(predicates_contains(
            "big.txt:1:1: error local/long-file: file big.txt has 5 lines",
        ));
    lighthouse(dir.path())
        .args(["decision", "test", "local/long-file"])
        .assert()
        .success()
        .stdout(predicates_contains("0 failure(s)"));
}

#[test]
fn decision_test_fails_when_an_example_does_not_hold() {
    let broken = LOCAL_RULE.replace("where: 'file.lines > 3", "where: 'file.lines > 30");
    let dir = local_project(&broken);
    lighthouse(dir.path())
        .args(["decision", "test", "local/long-file"])
        .assert()
        .code(1)
        .stdout(predicates_contains("FAIL [text] local/long-file long"));
}

#[test]
fn decision_test_rejects_unknown_ids() {
    let dir = local_project(LOCAL_RULE);
    lighthouse(dir.path())
        .args(["decision", "test", "local/nope"])
        .assert()
        .code(2);
}

#[test]
fn the_old_rule_command_names_the_decision_command() {
    let dir = project(6);
    lighthouse(dir.path())
        .args(["rule", "list", "--all"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "run `lighthouse decision list --all`",
        ));
}
