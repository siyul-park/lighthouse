//! `decision create` and `decision update` refuse a decision that makes
//! decisions contradict that did not, and write nothing.

use std::{fs, path::Path};

use predicates::prelude::*;
use serde_json::{Value, json};

use crate::{
    checks::{trust, write},
    contradictions::{lighthouse, project},
    mcp::Client,
};

/// An order decision in Go: the declarations of `first` come before the
/// others, with examples that pass.
fn order(name: &str, context: &str, first: &str) -> String {
    let (other, first_body, other_body) = match first {
        "type" => (
            "function",
            "package a\n\n// T is a type.\ntype T struct{}\n\n// F is a function.\nfunc F() {}\n",
            "package a\n\n// F is a function.\nfunc F() {}\n\n// T is a type.\ntype T struct{}\n",
        ),
        _ => (
            "type",
            "package a\n\n// F is a function.\nfunc F() {}\n\n// T is a type.\ntype T struct{}\n",
            "package a\n\n// T is a type.\ntype T struct{}\n\n// F is a function.\nfunc F() {}\n",
        ),
    };
    let kinds = if first == "type" {
        "symbol.kind == 'function' && file.symbols.exists(t, t.kind == 'type' && t.pos > symbol.pos)"
    } else {
        "symbol.kind == 'type' && file.symbols.exists(f, f.kind == 'function' && f.pos > symbol.pos)"
    };
    let edits = format!("[move(symbol, after(file.symbols.filter(t, t.kind == '{first}')[0]))]");
    format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/{name}\nspec:\n  title: A {first} comes before a {other}\n  context: {context}\n  scope: {{ subject: symbol, examples: exclude }}\n  requirement: A {first} MUST be declared above the {other}s of its file.\n  status: accepted\n  severity: warn\n  languages:\n    go: {{}}\n  check:\n    type: cel\n    select: symbol\n    where: \"{kinds}\"\n    message: declared out of order\n    identity:\n      subject: symbol.id\n  fix:\n    type: cel\n    safety: unsafe\n    edits: \"{edits}\"\n  examples:\n    - name: bad\n      language: go\n      kind: invalid\n      files: [{{ path: a.go, body: {other_body:?} }}]\n      fixed: [{{ path: a.go, body: {first_body:?} }}]\n      expect: [{{ line: 4 }}]\n    - name: good\n      language: go\n      kind: valid\n      files: [{{ path: a.go, body: {first_body:?} }}]\n"
    )
}

fn files_of(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir.join(".lighthouse"))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|e| {
            let sub = e.path();
            if sub.is_dir() && e.file_name() != "cache" {
                fs::read_dir(sub)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|f| f.path().display().to_string())
                    .collect()
            } else {
                vec![sub.display().to_string()]
            }
        })
        .filter(|name| !name.ends_with("authoring.db-lock") && !name.ends_with("lighthouse.db"))
        .collect();
    names.sort();
    names
}

/// How many signals the project's store holds.
fn signals(dir: &Path) -> usize {
    let out = lighthouse(dir)
        .args(["decision", "signals", "--all", "--format", "json"])
        .output()
        .unwrap();
    let told: Value = serde_json::from_slice(&out.stdout).unwrap();
    told["signals"].as_array().map_or(0, Vec::len)
}

#[test]
fn a_contradicting_create_is_refused_and_writes_nothing() {
    let Some(dir) = project(&[("types-first", &order("types-first", "T", "type"))]) else {
        return;
    };
    write(
        dir.path(),
        "functions-first.yaml",
        &order("functions-first", "F", "function"),
    );
    trust(dir.path());
    let config = fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap();
    let before = files_of(dir.path());
    let recorded = signals(dir.path());

    lighthouse(dir.path())
        .args(["decision", "create", "functions-first.yaml"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "`local/functions-first` was not written, it contradicts the decisions in force:",
        ))
        .stdout(predicate::str::contains(
            "contradicts [go] with local/types-first: on code shaped like a public type A, a public function B",
        ));

    assert_eq!(files_of(dir.path()), before, "no file");
    assert_eq!(signals(dir.path()), recorded, "no store record");
    assert_eq!(
        fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap(),
        config
    );
}

#[test]
fn a_compatible_create_is_written() {
    let Some(dir) = project(&[("types-first", &order("types-first", "T", "type"))]) else {
        return;
    };
    write(
        dir.path(),
        "types-also.yaml",
        &order("types-also", "Also", "type"),
    );
    trust(dir.path());

    lighthouse(dir.path())
        .args(["decision", "create", "types-also.yaml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("created local/types-also"));

    assert!(
        dir.path()
            .join(".lighthouse/decisions/types-also.yaml")
            .is_file()
    );
}

#[test]
fn an_update_that_introduces_a_contradiction_is_refused() {
    let Some(dir) = project(&[
        ("types-first", &order("types-first", "T", "type")),
        ("order", &order("order", "Before", "type")),
    ]) else {
        return;
    };
    write(
        dir.path(),
        "reversed.yaml",
        &order("order", "After", "function"),
    );
    trust(dir.path());
    let path = dir.path().join(".lighthouse/decisions/order.yaml");
    let before = fs::read_to_string(&path).unwrap();

    lighthouse(dir.path())
        .args(["decision", "update", "reversed.yaml"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "contradicts [go] with local/types-first",
        ));

    assert_eq!(fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn an_update_that_keeps_a_contradiction_it_had_is_not_refused() {
    let Some(dir) = project(&[
        ("types-first", &order("types-first", "T", "type")),
        ("order", &order("order", "Before", "function")),
    ]) else {
        return;
    };
    write(
        dir.path(),
        "reworded.yaml",
        &order("order", "Reworded", "function"),
    );
    trust(dir.path());

    lighthouse(dir.path())
        .args(["decision", "update", "reworded.yaml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("updated local/order"));

    let written = fs::read_to_string(dir.path().join(".lighthouse/decisions/order.yaml")).unwrap();
    assert!(written.contains("Reworded"), "{written}");
}

#[test]
fn an_update_that_reopens_a_settled_contradiction_is_refused() {
    let superseding = order("order", "Before", "function").replace(
        "  status: accepted\n",
        "  status: accepted\n  supersedes: [local/types-first]\n",
    );
    let superseded =
        order("types-first", "T", "type").replace("  status: accepted\n", "  status: superseded\n");
    let Some(dir) = project(&[("types-first", &superseded), ("order", &superseding)]) else {
        return;
    };
    write(
        dir.path(),
        "unsettled.yaml",
        &order("order", "Unsettled", "function"),
    );
    trust(dir.path());
    let path = dir.path().join(".lighthouse/decisions/order.yaml");
    let before = fs::read_to_string(&path).unwrap();

    lighthouse(dir.path())
        .args(["decision", "update", "unsettled.yaml"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "contradicts [go] with local/types-first",
        ));

    assert_eq!(fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn the_tool_refuses_as_the_command_does() {
    let Some(dir) = project(&[("types-first", &order("types-first", "T", "type"))]) else {
        return;
    };
    let document = order("functions-first", "F", "function");
    write(dir.path(), "functions-first.yaml", &document);
    trust(dir.path());
    let before = files_of(dir.path());
    let on_the_command_line = lighthouse(dir.path())
        .args(["decision", "create", "functions-first.yaml"])
        .output()
        .unwrap();
    let mut client = Client::start(dir.path());

    let refused = client.tool("decision_create", json!({ "document": document }));

    let message = refused.unwrap_err();
    assert_eq!(
        format!("{message}\n"),
        String::from_utf8_lossy(&on_the_command_line.stdout)
    );
    assert_eq!(files_of(dir.path()), before);
}
