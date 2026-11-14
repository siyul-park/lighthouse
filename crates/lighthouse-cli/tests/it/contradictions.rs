//! `decision test` and `spec validate` find the contradictions among the
//! decisions they load, through the same search a run makes.

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

use crate::checks::{home, trust, write};

const TYPES_FIRST: &str = "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/types-first\nspec:\n  title: Types come first\n  context: Readers meet one kind of declaration before the other.\n  scope:\n    subject: symbol\n  requirement: A type MUST be declared above the functions of its file.\n  severity: warn\n  check:\n    type: cel\n    select: symbol\n    where: \"symbol.kind == 'function' && file.symbols.exists(t, t.kind == 'type' && t.pos > symbol.pos)\"\n    message: \"declared out of order\"\n    identity:\n      subject: symbol.id\n  fix:\n    type: cel\n    safety: unsafe\n    edits: \"[move(symbol, after(file.symbols.filter(t, t.kind == 'type')[0]))]\"\n  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{ path: a.txt, body: x }]\n      fixed: [{ path: a.txt, body: y }]\n      expect: [{ line: 1 }]\n    - name: good\n      language: text\n      kind: valid\n      files: [{ path: a.txt, body: x }]\n";
const FUNCTIONS_FIRST: &str = "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/functions-first\nspec:\n  title: Functions come first\n  context: Readers meet one kind of declaration before the other.\n  scope:\n    subject: symbol\n  requirement: A function MUST be declared above the types of its file.\n  severity: warn\n  check:\n    type: cel\n    select: symbol\n    where: \"symbol.kind == 'type' && file.symbols.exists(f, f.kind == 'function' && f.pos > symbol.pos)\"\n    message: \"declared out of order\"\n    identity:\n      subject: symbol.id\n  fix:\n    type: cel\n    safety: unsafe\n    edits: \"[move(symbol, after(file.symbols.filter(f, f.kind == 'function')[0]))]\"\n  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{ path: a.txt, body: x }]\n      fixed: [{ path: a.txt, body: y }]\n      expect: [{ line: 1 }]\n    - name: good\n      language: text\n      kind: valid\n      files: [{ path: a.txt, body: x }]\n";
const KINDS_FIRST: &str = "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/kinds-first\nspec:\n  title: One kind comes first\n  context: Readers meet one kind of declaration before the other.\n  scope:\n    subject: symbol\n  requirement: The `first` kind of declaration MUST be declared above the other.\n  severity: warn\n  options:\n    properties:\n      first:\n        type: string\n        enum: [type, function]\n        default: type\n        description: The kind of declaration that comes first.\n    additionalProperties: false\n  check:\n    type: cel\n    select: symbol\n    where: \"symbol.kind != options.first && (symbol.kind == 'type' || symbol.kind == 'function') && file.symbols.exists(f, f.kind == options.first && f.pos > symbol.pos)\"\n    message: \"declared out of order\"\n    identity:\n      subject: symbol.id\n  fix:\n    type: cel\n    safety: unsafe\n    edits: \"[move(symbol, after(file.symbols.filter(f, f.kind == options.first)[0]))]\"\n  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{ path: a.txt, body: x }]\n      fixed: [{ path: a.txt, body: y }]\n      expect: [{ line: 1 }]\n    - name: good\n      language: text\n      kind: valid\n      files: [{ path: a.txt, body: x }]\n";

fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir).envs(home(dir));
    cmd
}

/// A Go project with the local decisions given, and the Go plugin built from
/// source; `None` when no Go toolchain is available.
fn project(decisions: &[(&str, &str)]) -> Option<TempDir> {
    let plugin = lighthouse_test_support::lang_go()?;
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "go.mod", "module example.com/app\n");
    write(
        dir.path(),
        "api.go",
        "package app\n\n// Open opens.\nfunc Open() {}\n",
    );
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-go\", path = {:?} }}]\n",
            plugin.to_str().unwrap()
        )),
    );
    for (name, text) in decisions {
        write(
            dir.path(),
            &format!(".lighthouse/decisions/{name}.yaml"),
            text,
        );
    }
    trust(dir.path());
    Some(dir)
}

#[test]
fn decision_test_fails_on_decisions_that_no_order_satisfies() {
    let Some(dir) = project(&[
        ("types-first", TYPES_FIRST),
        ("functions-first", FUNCTIONS_FIRST),
    ]) else {
        return;
    };

    lighthouse(dir.path())
        .args(["decision", "test", "local/types-first"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "contradiction in go: on code shaped like a public type A, a public function B, no order satisfies local/functions-first, local/types-first",
        ));
}

#[test]
fn decision_test_says_what_its_search_could_not_do_in_full() {
    let Some(dir) = project(&[("types-first", TYPES_FIRST)]) else {
        return;
    };

    lighthouse(dir.path())
        .args(["decision", "test", "local/types-first"])
        .assert()
        .stdout(predicate::str::contains(
            "the search for contradictions among go decisions is incomplete",
        ));
}

#[test]
fn spec_validate_reports_a_contradiction_the_options_of_a_project_make() {
    let Some(dir) = project(&[("types-first", TYPES_FIRST), ("kinds-first", KINDS_FIRST)]) else {
        return;
    };
    let plugin = lighthouse_test_support::lang_go().unwrap();
    let preset = |options: &str| {
        format!(
            "apiVersion: lighthouse/v1alpha1\nkind: Project\nmetadata:\n  name: demo\nspec:\n  plugins: [{{ id: lang-go, path: {:?} }}]\n  rules:\n    local/types-first: warn\n    local/kinds-first:\n      level: warn\n      options: {{ first: {options} }}\n",
            plugin.to_str().unwrap()
        )
    };
    write(dir.path(), "compatible.yaml", &preset("type"));
    write(dir.path(), "contradicting.yaml", &preset("function"));

    lighthouse(dir.path())
        .args(["spec", "validate", "compatible.yaml"])
        .assert()
        .success();
    lighthouse(dir.path())
        .args(["spec", "validate", "contradicting.yaml"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "contradicting.yaml: go: on code shaped like a public type A, a public function B, no order satisfies local/kinds-first, local/types-first",
        ));
}
