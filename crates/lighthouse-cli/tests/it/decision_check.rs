//! `decision check`: a decision held to the decisions in force, through the
//! binary and the tool. A document is laid over the project in memory; no
//! file is written and nothing is recorded.

use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};

use crate::{
    checks::{decision, trust, write},
    contradictions::{FUNCTIONS_FIRST, TYPES_FIRST, lighthouse, project},
    mcp::{Client, text_project},
};

const LONG: &str = "  check:\n    type: cel\n    select: file\n    where: \"file.lines > 3\"\n    message: too long\n";

/// A local decision about long files, in the project's own decisions.
fn with_probe(dir: &Path) {
    write(dir, ".lighthouse/decisions/probe.yaml", &decision(LONG));
}

fn json_of(cmd: &mut Command) -> Value {
    let out = cmd.output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!("{e}: {}", String::from_utf8_lossy(&out.stdout));
    })
}

#[test]
fn a_contradicting_file_exits_one_and_prints_the_witness() {
    let Some(dir) = project(&[("types-first", TYPES_FIRST)]) else {
        return;
    };
    write(dir.path(), "functions-first.yaml", FUNCTIONS_FIRST);
    trust(dir.path());

    lighthouse(dir.path())
        .args(["decision", "check", "functions-first.yaml"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "contradicts [go] with local/types-first",
        ))
        .stdout(predicate::str::contains(
            "on code shaped like a public type A, a public function B",
        ));
    assert!(
        !dir.path()
            .join(".lighthouse/decisions/functions-first.yaml")
            .exists(),
        "a check never writes the document"
    );
}

#[test]
fn full_detail_adds_the_arrangements_of_the_witness() {
    let Some(dir) = project(&[("types-first", TYPES_FIRST)]) else {
        return;
    };
    write(dir.path(), "functions-first.yaml", FUNCTIONS_FIRST);
    trust(dir.path());
    let arrangements = |detail: &str| {
        let checked = json_of(lighthouse(dir.path()).args([
            "decision",
            "check",
            "functions-first.yaml",
            "--format",
            "json",
            "--detail",
            detail,
        ]));
        assert_eq!(checked["verdict"], "contradicts", "{checked}");
        assert_eq!(checked["contradicts"][0]["basis"], "proven");
        checked["contradicts"][0]["witness"]["arrangements"].clone()
    };

    assert_eq!(arrangements("compact"), Value::Null);
    assert!(!arrangements("full").as_array().unwrap().is_empty());
}

#[test]
fn a_decision_that_orders_nothing_is_independent_and_the_basis_is_stated() {
    let dir = text_project();
    write(dir.path(), "probe.yaml", &decision(LONG));
    trust(dir.path());

    let checked = json_of(lighthouse(dir.path()).args([
        "decision",
        "check",
        "probe.yaml",
        "--format",
        "json",
    ]));

    assert_eq!(checked["verdict"], "independent", "{checked}");
    assert_eq!(checked["basis"], "empirical");
    assert_eq!(checked["implied"], json!([]));
    assert_eq!(checked["contradicts"], json!([]));
    lighthouse(dir.path())
        .args(["decision", "check", "probe.yaml"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "local/probe: Independent (Empirical basis)",
        ));
}

#[test]
fn the_id_of_a_catalog_decision_is_checked_as_the_project_has_it() {
    let dir = text_project();
    with_probe(dir.path());
    trust(dir.path());

    let checked = json_of(lighthouse(dir.path()).args([
        "decision",
        "check",
        "local/probe",
        "--format",
        "json",
    ]));

    assert_eq!(checked["decision"], "local/probe");
    assert_eq!(checked["verdict"], "independent", "{checked}");
}

#[test]
fn a_document_on_stdin_is_checked() {
    let dir = text_project();
    trust(dir.path());

    lighthouse(dir.path())
        .args(["decision", "check", "-"])
        .write_stdin(decision(LONG))
        .assert()
        .success()
        .stdout(predicate::str::contains("local/probe: Independent"));
}

#[test]
fn a_name_that_is_a_file_and_a_decision_is_a_usage_error() {
    let dir = text_project();
    with_probe(dir.path());
    write(dir.path(), "local/probe", &decision(LONG));
    trust(dir.path());

    lighthouse(dir.path())
        .args(["decision", "check", "local/probe"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("both a file and a decision"));
    lighthouse(dir.path())
        .args(["decision", "check", "nothing/here"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("neither a file nor a decision"));
}

#[test]
fn a_search_that_cannot_finish_exits_three() {
    let Some(dir) = project(&[("types-first", TYPES_FIRST)]) else {
        return;
    };

    lighthouse(dir.path())
        .args(["decision", "check", "local/types-first"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("incomplete: "));
}

#[test]
fn a_bundled_decision_is_independent_or_names_what_was_not_searched() {
    let dir = text_project();
    trust(dir.path());

    let out = lighthouse(dir.path())
        .args([
            "decision",
            "check",
            "design/top-down-order",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    let checked: Value = serde_json::from_slice(&out.stdout).unwrap();

    assert_eq!(checked["decision"], "design/top-down-order");
    assert_eq!(checked["tier"], "O");
    let incomplete = !checked["incomplete"].as_array().unwrap().is_empty();
    assert_eq!(out.status.code(), Some(if incomplete { 3 } else { 0 }));
    assert_eq!(checked["contradicts"], json!([]), "{checked}");
}

#[test]
fn the_tool_answers_as_the_command_does_and_records_nothing() {
    let dir = text_project();
    trust(dir.path());
    write(dir.path(), "probe.yaml", &decision(LONG));
    let before = listing(dir.path());
    let mut client = Client::start(dir.path());

    let reply = client.request(
        "tools/call",
        json!({ "name": "decision_check", "arguments": { "document": decision(LONG) } }),
    );

    let structured = &reply["result"]["structuredContent"];
    let on_the_command_line = json_of(lighthouse(dir.path()).args([
        "decision",
        "check",
        "probe.yaml",
        "--format",
        "json",
    ]));
    assert_eq!(structured, &on_the_command_line);
    assert!(!dir.path().join(".lighthouse/lighthouse.db").exists());
    assert_eq!(listing(dir.path()), before);
}

/// The files under the project's own decisions and its store.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir.join(".lighthouse"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "cache")
        .collect();
    names.sort();
    names
}
