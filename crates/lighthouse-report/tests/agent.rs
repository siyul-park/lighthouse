use std::collections::BTreeMap;

use lighthouse_model::{Diagnostic, Fingerprint, Incomplete, Position, Severity, Span};
use lighthouse_report::{Briefing, Format, render, render_with};
use lighthouse_spec::Catalog;
use serde_json::{Value, json};

fn span(line: u32) -> Span {
    Span {
        start: Position { line, col: 1 },
        end: Position {
            line: line + 8,
            col: 2,
        },
    }
}

fn coupling() -> Diagnostic {
    let mut d = Diagnostic::new(
        "design/coupling-signal",
        Severity::Warn,
        "function coordinates 14 collaborators in its package",
        "internal/jit/compile.go",
        span(350),
        Fingerprint::of("design/coupling-signal", "jit::compile#function", ""),
    );
    d.symbol = Some("jit::compile#function".to_owned());
    d.evidence = json!({ "fan_in": 2, "fan_out": 14, "callees": ["a", "b c"] });
    d
}

fn helper() -> Diagnostic {
    let mut d = Diagnostic::new(
        "design/private-helper-callers",
        Severity::Review,
        "private helper has one caller",
        "src/lib.rs",
        span(7),
        Fingerprint::of("design/private-helper-callers", "m::helper#function", ""),
    );
    d.symbol = Some("m::helper#function".to_owned());
    d
}

fn unknown_rule() -> Diagnostic {
    Diagnostic::new(
        "acme/custom",
        Severity::Error,
        "custom finding",
        "a.txt",
        span(1),
        Fingerprint::of("acme/custom", "a.txt", ""),
    )
}

fn facts(findings: &[Diagnostic]) -> BTreeMap<Fingerprint, Value> {
    let languages = ["go", "rust", "text"];
    findings
        .iter()
        .zip(languages)
        .map(|(d, language)| (d.fingerprint.clone(), json!({ "language": language })))
        .collect()
}

fn gap() -> Vec<Incomplete> {
    vec![Incomplete {
        path: Some("broken.go".into()),
        reason: "type error".to_owned(),
    }]
}

#[test]
fn agent_text_gives_each_finding_everything_needed_to_act() {
    let findings = [coupling(), helper(), unknown_rule()];
    let facts = facts(&findings);
    let briefing = Briefing {
        catalog: Some(Catalog::bundled()),
        facts: Some(&facts),
        suppressed: 2,
    };
    insta::assert_snapshot!(render_with(Format::Agent, &findings, &gap(), &briefing));
}

#[test]
fn agent_json_is_one_tagged_record_per_line() {
    let findings = [coupling(), helper(), unknown_rule()];
    let facts = facts(&findings);
    let briefing = Briefing {
        catalog: Some(Catalog::bundled()),
        facts: Some(&facts),
        suppressed: 2,
    };
    let out = render_with(Format::AgentJson, &findings, &gap(), &briefing);
    let records: Vec<Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let kinds: Vec<_> = records
        .iter()
        .map(|r| r["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["finding", "finding", "finding", "incomplete", "summary"]
    );

    let coupling = &records[0];
    assert_eq!(coupling["rule"], "design/coupling-signal");
    assert_eq!(coupling["tier"], "heuristic");
    assert_eq!(coupling["symbol"], "jit::compile#function");
    assert_eq!(coupling["location"]["line"], 350);
    assert_eq!(coupling["evidence"]["fan_out"], 14);
    assert!(coupling["requirement"].as_str().unwrap().contains("fan"));
    assert!(coupling.get("resolve").is_none());
    assert_eq!(coupling["expected"]["language"], "go");

    let review = &records[1];
    assert_eq!(review["severity"], "review");
    assert_eq!(review["tier"], "heuristic");
    let command = review["resolve"]["command"].as_str().unwrap();
    assert!(
        command.starts_with("lighthouse review resolve "),
        "{command}"
    );
    assert!(command.contains(review["fingerprint"].as_str().unwrap()));
    assert_eq!(review["resolve"]["verdicts"]["deferred"], json!(["none"]));

    let custom = &records[2];
    assert_eq!(custom["tier"], "mechanical");
    assert!(custom.get("requirement").is_none());
    assert!(custom.get("expected").is_none());

    let summary = &records[4];
    assert_eq!(summary["errors"], 1);
    assert_eq!(summary["reviews"], 1);
    assert_eq!(summary["suppressed"], 2);
}

#[test]
fn agent_formats_state_a_clean_run() {
    let text = render(Format::Agent, &[], &[]);
    assert_eq!(
        text,
        "summary: 0 error, 0 warn, 0 review, 0 incomplete, 0 suppressed\n"
    );
    let json = render(Format::AgentJson, &[], &[]);
    let summary: Value = serde_json::from_str(json.trim()).unwrap();
    assert_eq!(summary["type"], "summary");
}

#[test]
fn agent_excerpts_stay_bounded() {
    let findings = [coupling()];
    let facts = facts(&findings);
    let briefing = Briefing {
        catalog: Some(Catalog::bundled()),
        facts: Some(&facts),
        suppressed: 0,
    };
    let out = render_with(Format::AgentJson, &findings, &[], &briefing);
    let record: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    let excerpt = record["expected"]["excerpt"].as_str().unwrap();
    assert!(excerpt.lines().count() <= 13, "{excerpt}");
    assert!(excerpt.chars().count() <= 604, "{excerpt}");
}

#[test]
fn format_parses_agent_names() {
    assert_eq!("agent".parse::<Format>().unwrap(), Format::Agent);
    assert_eq!("agent-json".parse::<Format>().unwrap(), Format::AgentJson);
}
