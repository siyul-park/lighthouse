//! `command` checks and decisions that are not in force, through the binary,
//! over a text project with project-local decisions.

use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

pub(crate) fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir)
        .env("LIGHTHOUSE_HOME", dir.join(".home"))
        .env_remove("LIGHTHOUSE_TRUST");
    cmd
}

pub(crate) fn trust(dir: &Path) {
    lighthouse(dir).args(["trust", "--yes"]).assert().success();
}

pub(crate) fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A local decision `local/probe` with `extra` after its spec lines.
pub(crate) fn decision(extra: &str) -> String {
    format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/probe\nspec:\n  title: Probe\n  context: A probe.\n  scope: {{ subject: file }}\n  requirement: A file MUST NOT say hello.\n  severity: error\n{extra}  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{{ path: bad.txt, body: hello }}]\n      expect: [{{ line: 1 }}]\n    - name: good\n      language: text\n      kind: valid\n      files: [{{ path: good.txt, body: bye }}]\n"
    )
}

pub(crate) fn project(rule: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\", \"local\"]\n[rules]\n\"local/probe\" = \"error\"\n",
        ),
    );
    write(dir.path(), ".lighthouse/decisions/probe.yaml", rule);
    write(dir.path(), "a.txt", "hello\n");
    write(dir.path(), "bad.txt", "hello\n");
    dir
}

const LINT: &str = "if grep -q hello \"$1\"; then echo \"$1:1: says hello\"; exit 1; fi\n";

const BY_SCRIPT: &str = "  check:\n    type: command\n    argv: [sh, tools/lint.sh, \"{file}\"]\n";

fn scripted() -> TempDir {
    let dir = project(&decision(BY_SCRIPT));
    write(dir.path(), "tools/lint.sh", LINT);
    dir
}

#[test]
fn command_check_runs_only_in_a_trusted_project_and_reports_its_findings() {
    let dir = scripted();

    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(3)
        .stdout(predicate::str::contains("not trusted"));

    trust(dir.path());
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "a.txt:1:1: error local/probe: says hello",
        ));
}

#[test]
fn command_check_trust_covers_the_script_an_argument_names() {
    let dir = scripted();
    trust(dir.path());
    lighthouse(dir.path()).arg("check").assert().code(1);

    write(
        dir.path(),
        "tools/lint.sh",
        "echo \"$1:1: changed\"; exit 1\n",
    );

    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(3)
        .stdout(predicate::str::contains("not trusted"));
    trust(dir.path());
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(predicate::str::contains("changed"));
}

#[test]
fn command_check_exit_codes_and_empty_findings_are_execution_errors() {
    let dir = project(&decision(
        "  check:\n    type: command\n    argv: [sh, -c, \"exit 2\", sh, \"{file}\"]\n",
    ));
    trust(dir.path());
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(3)
        .stdout(predicate::str::contains("exited 2"));

    // Exit 1 says it found something; printing nothing contradicts that.
    let silent = project(&decision(
        "  check:\n    type: command\n    argv: [sh, -c, \"exit 1\", sh, \"{file}\"]\n",
    ));
    trust(silent.path());
    lighthouse(silent.path())
        .arg("check")
        .assert()
        .code(3)
        .stdout(predicate::str::contains("printed no finding"));
}

#[test]
fn command_check_output_past_the_cap_is_incomplete_not_clean() {
    let dir = project(&decision(
        "  check:\n    type: command\n    argv: [sh, -c, \"yes finding | head -c 2000000; exit 1\", sh, \"{file}\"]\n",
    ));
    trust(dir.path());
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(3)
        .stdout(predicate::str::contains("output cap"));
}

#[test]
fn a_deprecated_decision_is_not_enforced_and_does_not_break_the_run() {
    let dir = project(&decision(
        "  status: deprecated\n  check:\n    type: cel\n    select: file\n    where: file.lines > 0\n    message: m\n",
    ));
    lighthouse(dir.path()).arg("check").assert().success();
}

#[test]
fn a_proposed_decision_is_tested_by_decision_test_but_not_enforced() {
    let dir = project(&decision(
        "  status: proposed\n  check:\n    type: cel\n    select: file\n    where: file.path.startsWith(\"bad\")\n    message: m\n",
    ));
    // Nothing in the project is reported, though bad.txt would break the rule.
    lighthouse(dir.path()).arg("check").assert().success();
    lighthouse(dir.path())
        .args(["decision", "test", "local/probe"])
        .assert()
        .success();
}
