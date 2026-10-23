//! `command` checks and decisions that are not in force, through the binary,
//! over a text project with project-local decisions.

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// The user-level directories of a process run in `dir`: kept inside the
/// project's own scratch space, so that a test never reads or writes the real
/// home (the user's trust and `config.toml`, plugins and caches).
pub(crate) fn home(dir: &Path) -> [(&'static str, PathBuf); 3] {
    ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"].map(|name| (name, dir.join(".home")))
}

/// The user-level directories a session reads in this process (the user's
/// file, trust and plugins), moved into `home` while the guard lives, so that a
/// test that opens a session never reads the real ones.
pub(crate) struct UserHome(Vec<(&'static str, Option<OsString>)>);

impl UserHome {
    const NAMES: [&'static str; 2] = ["XDG_CONFIG_HOME", "XDG_DATA_HOME"];

    pub(crate) fn at(home: &Path) -> Self {
        let saved = Self::NAMES.map(|name| (name, env::var_os(name)));
        for name in Self::NAMES {
            // SAFETY: the other tests of this binary give the programs they
            // start user-level directories of their own.
            unsafe { env::set_var(name, home) };
        }
        Self(saved.into())
    }
}

impl Drop for UserHome {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            // SAFETY: as in `at`.
            match value {
                Some(value) => unsafe { env::set_var(name, value) },
                None => unsafe { env::remove_var(name) },
            }
        }
    }
}

/// The binary in `dir`, with the user-level directories inside it and no CI
/// override: nothing leaks between tests.
pub(crate) fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir)
        .envs(home(dir))
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

const BY_SCRIPT: &str =
    "  check:\n    type: command\n    command: [sh, tools/lint.sh, \"{file}\"]\n";

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

/// The outcome of `lighthouse check` over a project whose command check is
/// `check` (lines under the `check:` key), as the exit code and the report.
fn checked(check: &str) -> (i32, String) {
    checked_at("file", check)
}

fn checked_at(subject: &str, check: &str) -> (i32, String) {
    let rule = decision(&format!("  check:\n    type: command\n{check}"));
    let dir = project(&rule.replace("subject: file", &format!("subject: {subject}")));
    trust(dir.path());
    let out = lighthouse(dir.path()).arg("check").output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn a_program_ends_as_its_exit_code_and_its_output_say_for_every_exit_class() {
    let table: [(&str, &str, i32, &str); 7] = [
        // Findings by lines: exit 0 ran and found nothing; a code of `success` that
        // reports nothing is an error, not clean.
        (
            "command: [sh, -c, \"exit 0\", sh, \"{file}\"]\n",
            "no codes",
            0,
            "",
        ),
        (
            "command: [sh, -c, \"exit 1\", sh, \"{file}\"]\n",
            "1 prints nothing",
            3,
            "exited 1 and reported nothing",
        ),
        (
            "command: [sh, -c, \"echo \\\"$1:1: found\\\"; exit 1\", sh, \"{file}\"]\n",
            "1 prints",
            1,
            "found",
        ),
        // Another code is an error: the analysis is incomplete.
        (
            "command: [sh, -c, \"exit 2\", sh, \"{file}\"]\n",
            "2 is an error",
            3,
            "exited 2",
        ),
        // The codes can be remapped.
        (
            "command: [sh, -c, \"echo \\\"$1:1: found\\\"; exit 2\", sh, \"{file}\"]\n    success: [0, 2]\n",
            "2 remapped",
            1,
            "found",
        ),
        (
            "command: [sh, -c, \"exit 1\", sh, \"{file}\"]\n    success: [0]\n",
            "1 not ran",
            3,
            "exited 1",
        ),
        // A program that outlives its timeout is an error too.
        (
            "command: [sleep, \"5\"]\n    timeout: 1s\n",
            "timeout",
            3,
            "timed out",
        ),
    ];

    for (check, what, code, shown) in table {
        let (got, report) = checked(&format!("    {check}"));

        assert_eq!(got, code, "{what}: {report}");
        assert!(report.contains(shown), "{what}: {report}");
    }
}

#[test]
fn a_status_program_has_exactly_one_finding_when_it_fails_and_none_when_it_passes() {
    let failing = "    command: [sh, -c, \"echo broken >&2; exit 4\"]\n    output: status\n";
    let passing = "    command: [sh, -c, \"exit 0\"]\n    output: status\n";

    let (code, report) = checked_at("project", failing);
    let (clean, nothing) = checked_at("project", passing);

    assert_eq!(code, 1, "{report}");
    assert_eq!(report.matches("local/probe").count(), 1, "{report}");
    assert!(report.contains("broken"), "{report}");
    assert!(report.contains("1 error"), "{report}");
    assert_eq!(clean, 0, "{nothing}");
}

#[test]
fn a_line_that_is_only_a_path_places_the_finding_at_line_one_once_however_it_is_named() {
    // A tool that follows module declarations names a file once for each way
    // it reached it, and may name it by the path the file system resolves.
    let script = "printf '%s/a.txt\\n%s/a.txt\\n' \"$(pwd -P)\" \"$(pwd -P)\"; echo a.txt; exit 1";
    let (code, report) = checked_at(
        "project",
        &format!("    command: [sh, -c, {script:?}]\n    files: [\"a.txt\"]\n"),
    );

    assert_eq!(code, 1, "{report}");
    assert_eq!(report.matches("a.txt:1:1").count(), 1, "{report}");
    assert!(
        report.contains("Probe"),
        "the title is the message: {report}"
    );
}

#[test]
fn a_secret_a_check_prints_never_reaches_a_finding() {
    let rule = decision(
        "  check:\n    type: command\n    command: [sh, -c, \"echo \\\"$1:1: key $TOKEN\\\"; exit 1\", sh, \"{file}\"]\n    env: { TOKEN: \"${LIGHTHOUSE_CHECK_TEST_SECRET}\" }\n",
    );
    let dir = project(&rule);
    trust(dir.path());

    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_CHECK_TEST_SECRET", "sk-in-a-finding")
        .args(["check", "--format", "json"])
        .output()
        .unwrap();

    let report = String::from_utf8_lossy(&out.stdout);
    assert!(
        report.contains("key ${LIGHTHOUSE_CHECK_TEST_SECRET}"),
        "{report}"
    );
    assert!(!report.contains("sk-in-a-finding"), "{report}");
}

#[test]
fn command_check_gets_the_same_environment_as_a_fix_command() {
    let dir = project(&decision(
        "  check:\n    type: command\n    command: [sh, -c, \"test -n \\\"$HOME\\\" && test -n \\\"$LIGHTHOUSE_API_VERSION\\\" && test -z \\\"$LIGHTHOUSE_TEST_SECRET\\\" && echo \\\"$1:1: has the contract\\\"; exit 1\", sh, \"{file}\"]\n",
    ));
    trust(dir.path());
    lighthouse(dir.path())
        .env("LIGHTHOUSE_TEST_SECRET", "leaked")
        .arg("check")
        .assert()
        .code(1)
        .stdout(predicate::str::contains("has the contract"));
}

#[test]
fn command_check_output_past_the_cap_is_incomplete_not_clean() {
    let dir = project(&decision(
        "  check:\n    type: command\n    command: [sh, -c, \"yes finding | head -c 2000000; exit 1\", sh, \"{file}\"]\n",
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

#[test]
fn a_fact_of_a_file_its_provider_does_not_state_is_an_error_naming_the_file_and_the_fact() {
    let (code, report) = checked_at(
        "project",
        "    command: [sh, -c, \"exit 0\", sh, \"{file.edition}\", \"{files}\"]\n    files: [\"a.txt\"]\n",
    );

    assert_eq!(code, 3, "{report}");
    assert!(report.contains("`a.txt` has no `edition` fact"), "{report}");
}
