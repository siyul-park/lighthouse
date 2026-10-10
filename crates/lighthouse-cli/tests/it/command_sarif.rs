//! `command` checks that read a SARIF log, through the binary, with a fake
//! tool: a script that prints a fixed document, as a wrapped linter would.

use std::path::Path;

use serde_json::{Value, json};
use tempfile::TempDir;

use super::checks::{lighthouse, project, trust, write};

/// The decision that wraps the tool, once over the project.
fn wrapping(select: &str) -> String {
    format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/probe\nspec:\n  title: Probe\n  context: A probe.\n  scope: {{ subject: project }}\n  requirement: The tool MUST find nothing.\n  severity: error\n  check:\n    type: command\n    argv: [sh, tools/lint.sh]\n    batch: all\n    output: sarif\n{select}  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{{ path: bad.txt, body: hello }}]\n      expect: [{{ line: 1 }}]\n    - name: good\n      language: text\n      kind: valid\n      files: [{{ path: good.txt, body: bye }}]\n"
    )
}

const SCRIPT: &str = "sed \"s#@ROOT@#$PWD#g\" tools/log.json\nexit 1\n";

fn location(uri: &str, base: Option<&str>, line: u32, col: u32) -> Value {
    let mut artifact = json!({ "uri": uri });
    if let Some(base) = base {
        artifact["uriBaseId"] = json!(base);
    }
    json!({ "physicalLocation": {
        "artifactLocation": artifact,
        "region": { "startLine": line, "startColumn": col },
    } })
}

fn result(rule: &str, level: &str, text: &str, at: Value) -> Value {
    json!({ "ruleId": rule, "level": level, "message": { "text": text }, "locations": [at] })
}

/// The log of the fake tool: one result per behaviour the reader has.
fn log(line_of_hello: u32, count: u32) -> Value {
    let wide = result(
        "errcheck",
        "error",
        &format!("Error return value of `os.Remove` is not checked ({count} callers)"),
        location("src/wide.txt", Some("%SRCROOT%"), 1, 4),
    );
    let mut suppressed = result(
        "errcheck",
        "error",
        "ignored on purpose",
        location("src/b.txt", Some("%SRCROOT%"), 1, 1),
    );
    suppressed["suppressions"] = json!([{ "kind": "inSource", "justification": "known" }]);
    let mut by_template = result(
        "style:rename",
        "warning",
        "",
        location("src/a.txt", None, line_of_hello, 1),
    );
    by_template["message"] = json!({ "id": "default", "arguments": ["foo", "bar"] });
    by_template["ruleIndex"] = json!(0);
    json!({
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": { "name": "fake", "rules": [{
                "id": "style:rename",
                "helpUri": "https://example.test/rename",
                "messageStrings": { "default": { "text": "Rename {0} to {1}" } },
            }] } },
            "originalUriBaseIds": { "%SRCROOT%": { "uri": "file://@ROOT@/" } },
            "columnKind": "utf16CodeUnits",
            "results": [
                wide,
                suppressed,
                by_template,
                result("govet:printf", "note", "a note", location("src/a.txt", None, 1, 1)),
                result("errcheck", "note", "a lesser one", location("src/a.txt", None, 1, 1)),
                result("unused", "error", "not selected", location("src/a.txt", None, 1, 1)),
                result("errcheck", "error", "elsewhere", location("file:///not/here.go", None, 1, 1)),
                json!({ "ruleId": "errcheck", "level": "error", "message": { "text": "project" } }),
            ],
        }],
    })
}

fn wrapped(select: &str, log: &Value) -> TempDir {
    let dir = project(&wrapping(select));
    write(dir.path(), "tools/lint.sh", SCRIPT);
    write(dir.path(), "tools/log.json", &log.to_string());
    write(dir.path(), "src/a.txt", "first\nhello\n");
    write(dir.path(), "src/b.txt", "b\n");
    write(dir.path(), "src/wide.txt", "é😀x\n");
    trust(dir.path());
    dir
}

/// The findings of `lighthouse check --format json`, one object per line.
fn check(dir: &Path) -> Vec<Value> {
    let out = lighthouse(dir)
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

#[test]
fn a_sarif_log_becomes_findings_with_the_decisions_severity_and_the_tools_evidence() {
    let select =
        "    select: { ruleIds: [errcheck, \"govet:*\", \"style:*\"], levels: [error, warning] }\n";
    let dir = wrapped(select, &log(2, 3));
    let all = check(dir.path());

    let messages: Vec<&str> = all.iter().filter_map(|d| d["message"].as_str()).collect();
    assert!(
        messages.iter().any(|m| m.starts_with("Error return value")),
        "{all:?}"
    );
    assert!(messages.contains(&"Rename foo to bar"), "{messages:?}");
    assert!(messages.contains(&"project"));
    for hidden in ["a note", "a lesser one", "not selected", "elsewhere"] {
        assert!(
            !messages.contains(&hidden),
            "{hidden} is selected out or outside"
        );
    }

    let wide = all.iter().find(|d| d["file"] == "src/wide.txt").unwrap();
    assert_eq!(wide["severity"], "error");
    // `é` is 2 bytes, the emoji 4: the 4th UTF-16 unit is the 7th byte column.
    assert_eq!(wide["span"]["start"]["col"], 7, "{wide}");
    let renamed = all
        .iter()
        .find(|d| d["message"] == "Rename foo to bar")
        .unwrap();
    assert_eq!(renamed["evidence"]["tool"], "fake");
    assert_eq!(
        renamed["evidence"]["helpUri"],
        "https://example.test/rename"
    );
    assert_eq!(renamed["evidence"]["level"], "warning");
    assert!(
        all.iter().all(|d| d["message"] != "ignored on purpose"),
        "a result the tool suppressed is not a finding"
    );
}

#[test]
fn a_result_the_tool_suppressed_in_the_code_is_an_in_source_suppression() {
    let dir = wrapped("", &log(2, 3));
    let out = lighthouse(dir.path())
        .args(["check", "--format", "sarif"])
        .output()
        .unwrap();
    let sarif: Value = serde_json::from_slice(&out.stdout).unwrap();
    let results = sarif["runs"][0]["results"].as_array().unwrap();
    let kept = results
        .iter()
        .find(|r| r["message"]["text"] == "ignored on purpose")
        .expect("suppressed results stay visible in SARIF");
    assert_eq!(kept["suppressions"][0]["kind"], "inSource");
    assert_eq!(kept["suppressions"][0]["justification"], "known");
}

#[test]
fn a_finding_keeps_its_identity_when_its_line_and_the_numbers_in_its_message_move() {
    let select = "    select: { ruleIds: [errcheck, \"style:*\"] }\n";
    let fingerprints = |line: u32, count: u32| {
        let dir = wrapped(select, &log(line, count));
        let mut found: Vec<(String, Value)> = check(dir.path())
            .into_iter()
            .filter(|d| d["file"] != ".")
            .map(|d| (d["file"].to_string(), d["fingerprint"].clone()))
            .collect();
        found.sort_by_key(|(file, print)| (file.clone(), print.to_string()));
        found
    };
    let before = fingerprints(2, 3);
    assert_eq!(before.len(), 3, "{before:?}");
    assert_eq!(before, fingerprints(1, 40));
}

#[test]
fn output_that_is_not_a_sarif_log_is_an_incomplete_analysis_not_a_clean_one() {
    for printed in [
        "echo not json",
        "echo '{\"version\":\"2.1.0\",\"runs\":[]}'",
        "true",
    ] {
        let dir = project(&wrapping(""));
        write(dir.path(), "tools/lint.sh", &format!("{printed}\nexit 1\n"));
        trust(dir.path());
        let out = lighthouse(dir.path()).arg("check").output().unwrap();
        assert_eq!(out.status.code(), Some(3), "{printed}");
        let shown = String::from_utf8_lossy(&out.stdout);
        assert!(
            shown.contains("SARIF") || shown.contains("found something"),
            "{shown}"
        );
    }
}
