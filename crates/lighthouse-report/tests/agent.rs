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
        Severity::Info,
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
        allowed: 1,
        ..Briefing::default()
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
        allowed: 1,
        ..Briefing::default()
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
    assert_eq!(coupling["authored"], "warn");
    assert_eq!(coupling["symbol"], "jit::compile#function");
    assert_eq!(coupling["location"]["line"], 350);
    assert_eq!(coupling["evidence"]["fan_out"], 14);
    assert!(coupling["requirement"].as_str().unwrap().contains("fan"));
    assert!(
        coupling["resolve"].is_object(),
        "a heuristic finding asks for a verdict whatever its severity"
    );
    assert_eq!(coupling["expected"]["language"], "go");

    let review = &records[1];
    assert_eq!(review["severity"], "info");
    assert_eq!(review["authored"], "info");
    let command = review["resolve"]["command"].as_str().unwrap();
    assert!(
        command.starts_with("lighthouse review resolve "),
        "{command}"
    );
    assert!(command.contains(&review["fingerprint"].as_str().unwrap()[..12]));
    assert!(review["resolve"].get("verdicts").is_none());

    let custom = &records[2];
    assert_eq!(custom["authored"], "error");
    assert!(
        custom.get("resolve").is_none(),
        "an error is not a review task"
    );
    assert!(custom.get("requirement").is_none());
    assert!(custom.get("expected").is_none());

    let summary = &records[4];
    assert_eq!(summary["errors"], 1);
    assert_eq!(summary["warnings"], 1);
    assert_eq!(summary["infos"], 1);
    assert_eq!(summary["reviews"], 2);
    assert_eq!(summary["suppressed"], 2);
    assert_eq!(summary["allowed"], 1);
    assert_eq!(summary["reasons"]["deferred"], json!(["none"]));
    assert_eq!(summary["reasons"]["rejected"][0], "false-positive");
}

#[test]
fn agent_formats_state_a_clean_run() {
    let text = render(Format::Agent, &[], &[]);
    assert_eq!(
        text,
        "summary: 0 error, 0 warn, 0 info, 0 review, 0 incomplete, 0 suppressed, 0 allowed\n"
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
        ..Briefing::default()
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

fn briefed<'a>(
    facts: &'a BTreeMap<Fingerprint, Value>,
    extra: impl FnOnce(Briefing<'a>) -> Briefing<'a>,
) -> Briefing<'a> {
    extra(Briefing {
        catalog: Some(Catalog::bundled()),
        facts: Some(facts),
        ..Briefing::default()
    })
}

fn records_of(out: &str) -> Vec<Value> {
    out.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn limit_keeps_the_most_severe_findings_and_counts_the_rest() {
    let findings = [helper(), coupling(), unknown_rule()];
    let facts = facts(&findings);
    let briefing = briefed(&facts, |b| Briefing {
        limit: Some(2),
        ..b
    });
    let text = render_with(Format::Agent, &findings, &[], &briefing);
    assert!(text.contains("acme/custom  error"), "{text}");
    assert!(text.contains("design/coupling-signal  warn"), "{text}");
    assert!(!text.contains("private-helper-callers  info"), "{text}");
    assert!(
        text.contains("... 1 more finding(s) not shown (raise --limit)"),
        "{text}"
    );
    assert!(
        text.contains("summary: 1 error, 1 warn, 1 info, 2 review"),
        "{text}"
    );
    assert!(
        text.contains("reasons:"),
        "a shown heuristic finding asks for a verdict: {text}"
    );

    let json = records_of(&render_with(Format::AgentJson, &findings, &[], &briefing));
    let kinds: Vec<_> = json.iter().map(|r| r["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["finding", "finding", "truncated", "summary"]);
    assert_eq!(json[2]["omitted"], 1);
    assert_eq!(
        json[0]["rule"], "design/coupling-signal",
        "original order is kept"
    );
    assert!(json[3]["reasons"].is_object());
}

#[test]
fn notes_say_why_a_finding_is_reported() {
    let findings = [unknown_rule()];
    let facts = facts(&findings);
    let notes = BTreeMap::from([(
        findings[0].fingerprint.clone(),
        "verdict expired: rule changed".to_owned(),
    )]);
    let briefing = briefed(&facts, |b| Briefing {
        notes: Some(&notes),
        ..b
    });
    let text = render_with(Format::Agent, &findings, &[], &briefing);
    assert!(
        text.contains("  note:        verdict expired: rule changed"),
        "{text}"
    );
    let json = records_of(&render_with(Format::AgentJson, &findings, &[], &briefing));
    assert_eq!(json[0]["note"], "verdict expired: rule changed");
}

#[test]
fn evidence_leaves_out_the_owner_symbol_and_cuts_long_values() {
    let mut d = coupling();
    d.evidence = json!({
        "symbol": "jit::compile#function",
        "callees": "x".repeat(400),
        "fan_in": 2,
    });
    let findings = [d];
    let facts = facts(&findings);
    let briefing = briefed(&facts, |b| b);
    let json = records_of(&render_with(Format::AgentJson, &findings, &[], &briefing));
    let evidence = &json[0]["evidence"];
    assert!(evidence.get("symbol").is_none());
    assert_eq!(evidence["fan_in"], 2);
    assert!(evidence["callees"].as_str().unwrap().len() < 140);
    let text = render_with(Format::Agent, &findings, &[], &briefing);
    assert!(!text.contains("symbol=jit"), "{text}");
}

#[test]
fn fingerprint_prefixes_grow_until_the_shown_findings_are_distinct() {
    let make = |raw: &str| {
        let mut d = unknown_rule();
        d.fingerprint = Fingerprint::from_raw(raw);
        d
    };
    let a = format!("{}1{}", "a".repeat(12), "0".repeat(51));
    let b = format!("{}2{}", "a".repeat(12), "0".repeat(51));
    let findings = [make(&a), make(&b)];
    let text = render(Format::Agent, &findings, &[]);
    assert!(
        text.contains(&format!("fingerprint: {}", &a[..13])),
        "{text}"
    );
    assert!(
        text.contains(&format!("fingerprint: {}", &b[..13])),
        "{text}"
    );
}

fn decision_with(examples: &str, tuning: &str) -> Catalog {
    let indented: String = examples.lines().map(|l| format!("  {l}\n")).collect();
    let empty = if examples.is_empty() { " []" } else { "" };
    let decision = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: p/a\n  labels:\n    lighthouse/pack: p\n    lighthouse/section: s\nspec:\n  title: A\n  intent: i\n  scope: {{ subject: symbol }}\n  requirement: A MUST b.\n  severity: info\n  check:\n    type: model\n{tuning}  examples:{empty}\n{indented}"
    );
    let files = std::collections::BTreeMap::from([
        (
            "p/pack.yaml".to_owned(),
            "apiVersion: lighthouse/v1alpha1\nkind: Pack\nmetadata:\n  name: p\nspec:\n  title: P\n  intro: x\n  sections:\n    - name: s\n      title: S\n      intro: x\n      decisions: [a]\n".to_owned(),
        ),
        ("p/s/a.yaml".to_owned(), decision),
    ]);
    Catalog::from_files(files).unwrap()
}

fn example(name: &str, kind: &str, canonical: bool, body: &str) -> String {
    format!(
        "  - name: {name}\n    language: go\n    kind: {kind}\n    canonical: {canonical}\n    files:\n      - path: a.go\n        body: |-\n          {body}\n"
    )
}

fn expected_basis(catalog: &Catalog, kind: &str) -> Option<(String, String)> {
    let mut d = unknown_rule();
    d.rule_id = "p/a".to_owned();
    d.file = "x.go".into();
    let findings = [d];
    let facts = BTreeMap::from([(
        findings[0].fingerprint.clone(),
        json!({ "language": "go", "kind": kind, "visibility": "private" }),
    )]);
    let briefing = Briefing {
        catalog: Some(catalog),
        facts: Some(&facts),
        ..Briefing::default()
    };
    let out = render_with(Format::AgentJson, &findings, &[], &briefing);
    let record = &records_of(&out)[0];
    let expected = record.get("expected")?;
    Some((
        expected["basis"].as_str()?.to_owned(),
        expected["name"]
            .as_str()
            .or(expected["source"].as_str())?
            .to_owned(),
    ))
}

#[test]
fn expected_structure_prefers_canonical_then_a_match_then_the_shortest_then_tuning() {
    let canonical = decision_with(
        &format!(
            "{}{}",
            example("short-valid", "valid", false, "x"),
            example("long-valid", "valid", true, "x\n          y\n          z")
        ),
        "",
    );
    assert_eq!(
        expected_basis(&canonical, "function"),
        Some(("canonical".to_owned(), "long-valid".to_owned()))
    );

    let matched = decision_with(
        &format!(
            "{}{}{}{}",
            example("plain-valid", "valid", false, "x"),
            example("method-valid", "valid", false, "x\n          y"),
            example("plain-invalid", "invalid", false, "x").replace(
                "canonical: false",
                "canonical: false\n    expect: [{ line: 1 }]"
            ),
            example("method-invalid", "invalid", false, "x").replace(
                "canonical: false",
                "canonical: false\n    expect: [{ line: 1 }]"
            ),
        ),
        "",
    );
    assert_eq!(
        expected_basis(&matched, "method"),
        Some(("matches kind=method".to_owned(), "method-valid".to_owned()))
    );
    assert_eq!(
        expected_basis(&matched, "function"),
        Some((
            "shortest valid example".to_owned(),
            "plain-valid".to_owned()
        ))
    );

    let tuned = decision_with(
        "",
        "  languages:\n    go:\n      tuning: Write it the Go way.\n",
    );
    assert_eq!(
        expected_basis(&tuned, "function"),
        Some(("tuning".to_owned(), "tuning".to_owned()))
    );
    assert_eq!(expected_basis(&decision_with("", ""), "function"), None);
}

#[test]
fn agent_report_has_the_records_the_json_lines_print() {
    let catalog = Catalog::bundled();
    let briefing = Briefing {
        catalog: Some(catalog),
        limit: Some(1),
        ..Briefing::default()
    };
    let findings = [unknown_rule(), helper()];
    let gaps = [Incomplete {
        path: None,
        reason: "plugin crashed".to_owned(),
    }];
    let report = lighthouse_report::agent_report(&findings, &gaps, &briefing);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0]["type"], "finding");
    assert_eq!(report.omitted, 1);
    assert_eq!(report.incomplete[0]["reason"], "plugin crashed");
    assert!(
        report.reasons.is_none(),
        "the finding that asks for a verdict was left out"
    );

    let all = lighthouse_report::agent_report(
        &findings,
        &[],
        &Briefing {
            catalog: Some(catalog),
            ..Briefing::default()
        },
    );
    assert_eq!(all.findings.len(), 2);
    assert_eq!(all.omitted, 0);
    assert!(all.reasons.is_some());
}

#[test]
fn needs_verdict_follows_the_tier_of_the_decision_and_not_the_severity() {
    let briefing = Briefing {
        catalog: Some(Catalog::bundled()),
        ..Briefing::default()
    };
    let mut heuristic = coupling();
    heuristic.severity = Severity::Error;
    assert!(briefing.needs_verdict(&heuristic), "heuristic at error");
    let mechanical = Diagnostic::new(
        "core/annotation-reason",
        Severity::Warn,
        "needs a reason",
        "src/lib.rs",
        span(1),
        Fingerprint::of("core/annotation-reason", "m", ""),
    );
    assert!(!briefing.needs_verdict(&mechanical), "mechanical at warn");
}
