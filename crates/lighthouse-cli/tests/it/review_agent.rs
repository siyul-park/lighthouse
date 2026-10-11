//! The agent formats of `check` over remembered findings: the compact and the
//! full shape, the resolve command each finding carries, and `--limit`.

use serde_json::Value;

use crate::review::{HELPER, lighthouse, records, rust_project, stdout};

#[test]
fn agent_format_groups_findings_and_agent_json_carries_the_same_groups() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let text = stdout(lighthouse(dir.path()).args(["check", "--format", "agent"]));
    for expected in [
        "design/no-panic info [review] \u{2014} Panic MUST be limited to impossible programmer errors",
        "  expected:\n    pub fn parse(text: &str) -> Result<u32, std::num::ParseIntError> {",
        "  evidence: function=run kind=unwrap\n",
        "  src/lib.rs:2:",
        "resolve: lighthouse review resolve <fingerprint> ",
        "summary: 0 error, 1 warn, 1 info, 2 review, 0 incomplete, 0 suppressed",
    ] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
    assert!(
        !text.contains("fingerprint:") && !text.contains("symbol="),
        "{text}"
    );

    let json = records(&stdout(lighthouse(dir.path()).args([
        "check",
        "--format",
        "agent-json",
    ])));
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
    let text =
        stdout(lighthouse(dir.path()).args(["check", "--format", "agent", "--detail", "full"]));
    for expected in [
        "design/no-panic  info  src/lib.rs:2:7",
        "  owner:       demo::run#function",
        "  requirement: Panic MUST be limited to impossible programmer errors",
        "  evidence:    function=run kind=unwrap",
        "  expected:    valid rust example `rust-result` (src/lib.rs)",
        "lighthouse review resolve ",
    ] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
    let json = records(&stdout(lighthouse(dir.path()).args([
        "check",
        "--format",
        "agent-json",
        "--detail",
        "full",
    ])));
    let findings: Vec<_> = json.iter().filter(|r| r["type"] == "finding").collect();
    assert_eq!(findings.len(), 2);
    assert_eq!(json.last().unwrap()["type"], "summary");
    lighthouse(dir.path())
        .args(["check", "--detail", "full"])
        .assert()
        .code(2)
        .stderr("lighthouse: --detail applies to the agent formats only\n");
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
fn the_resolve_command_of_an_agent_finding_runs_as_given() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let text = stdout(lighthouse(dir.path()).args(["check", "--format", "agent"]));
    let template = text
        .lines()
        .find_map(|l| l.strip_prefix("resolve: "))
        .unwrap();
    let json = records(&stdout(lighthouse(dir.path()).args([
        "check",
        "--format",
        "agent-json",
    ])));
    let helper = group(&json[0], "design/no-panic");
    let prefix = helper["files"]["src/lib.rs"][0][2].as_str().unwrap();
    let args: Vec<&str> = template
        .split_whitespace()
        .skip(1)
        .map(|a| match a {
            "<fingerprint>" => prefix,
            "<pass|fail|notApplicable>" => "fail",
            other => other,
        })
        .filter(|a| !["[--suppress", "<justification>]", "[--reason", "<text>]"].contains(a))
        .collect();
    lighthouse(dir.path()).args(args).assert().success();
    let history = records(&stdout(
        lighthouse(dir.path()).args(["review", "history", prefix, "--format", "json"]),
    ));
    assert_eq!(history[0]["wasAttributedTo"]["type"], "SoftwareAgent");
    assert_eq!(history[0]["judgment"], "fail");
    assert_eq!(history[0]["label"], "positive");
    assert_eq!(history[0]["fingerprint"].as_str().unwrap().len(), 64);
}

#[test]
fn limit_trims_agent_output_and_is_refused_for_other_formats() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let text = stdout(lighthouse(dir.path()).args(["check", "--format", "agent", "--limit", "1"]));
    assert!(text.contains("design/exported-doc warn"), "{text}");
    assert!(!text.contains("no-panic"), "{text}");
    assert!(text.contains("... 1 more finding(s) not shown"), "{text}");
    lighthouse(dir.path())
        .args(["check", "--limit", "1"])
        .assert()
        .code(2)
        .stderr("lighthouse: --limit applies to the agent formats only\n");
}
