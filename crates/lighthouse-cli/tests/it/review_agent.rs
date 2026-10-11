//! The JSON shapes of `check` over remembered findings: the compact and the
//! full detail, the resolve command each finding carries, and `--limit`.

use serde_json::Value;

use crate::review::{HELPER, lighthouse, records, rust_project, stdout};

#[test]
fn text_lists_one_line_per_finding_and_json_compact_groups_them() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let text = stdout(lighthouse(dir.path()).args(["check", "--format", "text"]));
    assert!(
        text.contains("src/lib.rs:2:7: info design/no-panic:"),
        "{text}"
    );
    assert!(
        text.contains("summary: 0 error, 1 warn, 1 info, 0 incomplete"),
        "{text}"
    );

    let json = records(&stdout(
        lighthouse(dir.path()).args(["check", "--format", "json"]),
    ));
    assert_eq!(json.len(), 1, "one object");
    let groups = json[0]["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(json[0]["counts"]["review"], 2);
    assert_eq!(json[0]["status"], "findings");
    assert!(json[0]["resolve"].is_string());
    let helper = group(&json[0], "design/no-panic");
    assert_eq!(helper["review"], true);
    let instance = &helper["files"]["src/lib.rs"][0];
    assert_eq!(instance[0], "2:7");
    assert_eq!(instance[2].as_str().unwrap().len(), 7);
}

#[test]
fn full_detail_keeps_one_record_per_finding() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let json = records(&stdout(
        lighthouse(dir.path()).args(["check", "--format", "json", "--detail", "full"]),
    ));
    assert_eq!(json.len(), 1, "one object");
    let findings = json[0]["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 2);
    assert!(findings.iter().all(|f| f["type"] == "finding"));
    let panic = findings
        .iter()
        .find(|f| f["rule"] == "design/no-panic")
        .unwrap();
    assert_eq!(panic["location"]["line"], 2);
    assert_eq!(panic["symbol"], "demo::run#function");
    assert_eq!(panic["evidence"]["kind"], "unwrap");
    assert_eq!(panic["expected"]["name"], "rust-result");
    assert!(panic["resolve"]["command"].is_string());
}

/// The group of `rule` in a compact report.
fn group<'a>(report: &'a Value, rule: &str) -> &'a Value {
    report["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["rule"] == rule)
        .unwrap_or_else(|| panic!("{rule} in {report}"))
}

#[test]
fn the_resolve_command_of_a_full_finding_runs_as_given() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let json = records(&stdout(
        lighthouse(dir.path()).args(["check", "--format", "json", "--detail", "full"]),
    ));
    let finding = json[0]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule"] == "design/no-panic")
        .unwrap();
    let prefix = finding["fingerprint"].as_str().unwrap()[..7].to_owned();
    let command = finding["resolve"]["command"].as_str().unwrap();
    let args: Vec<&str> = command
        .split_whitespace()
        .skip(1)
        .map(|a| match a {
            "<pass|fail|notApplicable>" => "fail",
            "agent:<id>" => "agent:tester",
            other => other,
        })
        .filter(|a| !["[--suppress", "<justification>]", "[--reason", "<text>]"].contains(a))
        .collect();
    lighthouse(dir.path()).args(args).assert().success();
    let history = records(&stdout(
        lighthouse(dir.path()).args(["review", "history", &prefix, "--format", "json"]),
    ));
    let event = &history[0]["events"][0];
    assert_eq!(event["wasAttributedTo"]["type"], "SoftwareAgent");
    assert_eq!(event["wasAttributedTo"]["id"], "tester");
    assert_eq!(event["judgment"], "fail");
    assert_eq!(event["label"], "positive");
    assert_eq!(event["fingerprint"].as_str().unwrap().len(), 64);
}

#[test]
fn limit_trims_the_output_and_counts_what_it_left_out() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let json = records(&stdout(
        lighthouse(dir.path()).args(["check", "--format", "json", "--limit", "1"]),
    ));
    assert_eq!(json[0]["groups"].as_array().unwrap().len(), 1);
    assert_eq!(group(&json[0], "design/exported-doc")["severity"], "warn");
    assert_eq!(json[0]["omitted"]["findings"], 1);

    let text = stdout(lighthouse(dir.path()).args(["check", "--format", "text", "--limit", "1"]));
    assert!(text.contains("design/exported-doc"), "{text}");
    assert!(!text.contains("no-panic"), "{text}");
    assert!(text.contains("1 more"), "{text}");
}
