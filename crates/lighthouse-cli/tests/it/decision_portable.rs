//! `lighthouse decision test` answers the same in any project: its examples
//! run in projects of their own, so the project that runs them, the plugins it
//! lists and whether it is trusted decide only what can run, never what fails.

use std::{fs, path::PathBuf};

use assert_cmd::assert::Assert;
use tempfile::TempDir;

use crate::{cache_fixtures::write, fix::lighthouse};

/// A project outside this repository's layout: the plugins installed as a
/// user installs them and listed, `extends` and whatever else `extra` sets.
fn project(plugins: &[(&str, PathBuf)], extends: &[&str], extra: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (_, plugin) in plugins {
        lighthouse_test_support::installed(&dir.path().join(".home"), plugin);
    }
    let listed: Vec<String> = plugins.iter().map(|(id, _)| format!("{id:?}")).collect();
    let extends: Vec<String> = extends.iter().map(|id| format!("{id:?}")).collect();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [{}]\nextends = [{}]\n{extra}",
            listed.join(", "),
            extends.join(", ")
        )),
    );
    dir
}

/// A project that lists only `lang-go`, `None` after a skip message when no
/// Go toolchain runs.
fn go_only(extra: &str) -> Option<TempDir> {
    let go = lighthouse_test_support::lang_go()?;
    Some(project(
        &[("lang-go", go)],
        &["design/recommended", "go/recommended"],
        extra,
    ))
}

fn stdout(assert: &Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

fn decision_test(dir: &TempDir, args: &[&str]) -> Assert {
    lighthouse(dir.path())
        .args(["decision", "test"])
        .args(args)
        .assert()
}

#[test]
fn a_project_with_only_the_go_plugin_has_no_failing_example() {
    let Some(dir) = go_only("") else { return };
    let ran = decision_test(&dir, &[]);
    let out = stdout(&ran);
    assert!(out.contains(" 0 failure(s)"), "{out}");
    ran.success();
}

#[test]
fn an_untrusted_project_skips_the_examples_of_command_checks() {
    let (Some(go), true) = (
        lighthouse_test_support::lang_go(),
        fs::metadata("/bin/sh").is_ok(),
    ) else {
        return;
    };
    let plugins = [
        ("lang-go", go),
        ("lang-markdown", lighthouse_test_support::lang_markdown()),
    ];
    let dir = project(&plugins, &["markdown/recommended"], "");
    let bin = dir.path().join("bin");
    write(&bin, "markdownlint-cli2", "#!/bin/sh\nexit 0\n");
    #[cfg(unix)]
    fs::set_permissions(
        bin.join("markdownlint-cli2"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let ran = lighthouse(dir.path())
        .env("PATH", path)
        .args(["decision", "test", "markdown/*"])
        .assert();
    let out = stdout(&ran);
    assert!(out.contains(" 0 failure(s)"), "{out}");
    assert!(out.contains("not trusted"), "{out}");
    ran.success();
}

/// The line `decision test` ends with, its counts.
fn summary(out: &str) -> &str {
    out.lines()
        .find(|line| line.starts_with("decision test:"))
        .unwrap_or_default()
}

#[test]
fn a_project_with_only_the_go_plugin_says_what_it_skipped_and_why() {
    let Some(dir) = go_only("") else { return };
    let ran = decision_test(&dir, &[]);
    let out = stdout(&ran);
    assert!(
        out.contains("SKIP plugin `lang-yaml` is not available: "),
        "the CEL of decisions needs the plugins of core/authoring: {out}"
    );
    assert!(
        out.contains("SKIP plugin `lang-python` is not available: "),
        "{out}"
    );
    assert_eq!(
        out.matches("plugin `lang-yaml`").count(),
        1,
        "a missing plugin is told once, not once per example: {out}"
    );
    ran.code(0);
}

#[test]
fn what_a_project_extends_or_sets_does_not_change_what_its_examples_do() {
    let (Some(bare), Some(go)) = (go_only(""), lighthouse_test_support::lang_go()) else {
        return;
    };
    let busy = project(
        &[("lang-go", go)],
        &["core/recommended", "design/strict", "go/recommended"],
        "generated = { files = [\"nothing/\"] }\n[rules]\n\"core/max-lines\" = \"off\"\n\"design/complexity\" = \"off\"\n[[overrides]]\nfiles = [\"**/*.yaml\"]\nrules = { \"core/max-len\" = \"off\" }\n",
    );

    let plain = stdout(&decision_test(&bare, &[]));
    let set = stdout(&decision_test(&busy, &[]));

    assert!(plain.contains(" 0 failure(s)"), "{plain}");
    assert_eq!(summary(&plain), summary(&set), "{set}");
}

/// A decision of the project's own layer (`.lighthouse/decisions`) with a variable its `where` never reads.
const UNUSED_VARIABLE: &str = "apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/short
spec:
  title: Notes stay short
  context: Notes are read in one glance.
  scope:
    subject: file
  requirement: A note MUST NOT exceed three lines.
  severity: warn
  check:
    type: cel
    select: file
    variables:
      - name: limit
        expression: '3'
    where: file.lines > 3
    message: too long
  examples:
  - name: long
    language: text
    kind: invalid
    files:
    - path: note.txt
      body: |-
        a
        b
        c
        d
    expect: [{ line: 1 }]
  - name: short
    language: text
    kind: valid
    canonical: true
    files:
    - path: note.txt
      body: a
";

#[test]
fn a_project_that_extends_core_authoring_has_the_cel_of_its_decisions_checked() {
    let (yaml, toml) = (
        lighthouse_test_support::lang_yaml(),
        lighthouse_test_support::lang_toml(),
    );
    let with = project(&[], &["core/authoring", "design/recommended"], "");
    let without = project(&[], &["design/recommended"], "");
    for dir in [&with, &without] {
        lighthouse_test_support::installed(&dir.path().join(".home"), &yaml);
        lighthouse_test_support::installed(&dir.path().join(".home"), &toml);
        write(
            dir.path(),
            ".lighthouse/decisions/short.yaml",
            UNUSED_VARIABLE,
        );
    }

    let found = lighthouse(with.path()).arg("check").assert();
    let missed = lighthouse(without.path()).arg("check").assert();

    let found = stdout(&found);
    assert!(
        found.contains("design/no-unused-symbol: limit is never used"),
        "{found}"
    );
    let missed = stdout(&missed);
    assert!(
        !missed.contains("limit is never used"),
        "no CEL is checked unless the project asks for it: {missed}"
    );
}

#[test]
fn extending_core_authoring_without_its_plugins_names_the_project_that_needs_them() {
    let dir = project(&[], &["core/authoring"], "");

    let failed = lighthouse(dir.path()).arg("check").assert().code(2);

    let said = String::from_utf8_lossy(&failed.get_output().stderr).into_owned();
    assert!(
        said.contains("unknown plugin `lang-yaml` (extended from core/authoring)"),
        "{said}"
    );
}

#[test]
fn init_makes_a_project_with_decisions_extend_core_authoring() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".lighthouse/decisions")).unwrap();
    let bare = tempfile::tempdir().unwrap();

    lighthouse(dir.path()).arg("init").assert().success();
    lighthouse(bare.path()).arg("init").assert().success();

    let written = fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap();
    assert!(written.contains("\"core/authoring\""), "{written}");
    let plain = fs::read_to_string(bare.path().join("lighthouse.toml")).unwrap();
    assert!(!plain.contains("core/authoring"), "{plain}");
}
