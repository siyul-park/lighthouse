use std::collections::BTreeMap;

use lighthouse_model::{Diagnostic, Fingerprint, Incomplete, Position, Severity, Span};
use lighthouse_report::{Briefing, Detail, Format, render, render_with, shown};
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
        "design/coupling",
        Severity::Warn,
        "function coordinates 14 collaborators in its package",
        "internal/jit/compile.go",
        span(350),
        Fingerprint::of("design/coupling", "jit::compile#function", ""),
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

/// An undocumented exported function: the symbol is named in the message, so
/// the finding is its position, its message and its fingerprint.
fn exported(index: usize, file: &str) -> Diagnostic {
    let symbol = format!("pkg::Func{index}#function");
    let mut d = Diagnostic::new(
        "design/exported-doc",
        Severity::Warn,
        format!("exported function Func{index} must have a doc comment"),
        file,
        span(index as u32 * 3 + 1),
        Fingerprint::of("design/exported-doc", &symbol, ""),
    );
    d.evidence = json!({ "symbol": symbol });
    d.symbol = Some(symbol);
    d
}

/// `count` undocumented exported functions spread over a few files.
fn flagged(count: usize) -> Vec<Diagnostic> {
    (0..count)
        .map(|i| exported(i, &format!("pkg/file{}.go", i % 4)))
        .collect()
}

/// Findings of three decisions, `count` in all: a large group of
/// undocumented exports, helpers with one caller, and coupling hubs.
fn mixed(count: usize) -> Vec<Diagnostic> {
    (0..count)
        .map(|i| match i % 5 {
            0..=2 => exported(i, &format!("internal/store/file{}.go", i % 7)),
            3 => {
                let symbol = format!("store::helper{i}#function");
                let mut d = Diagnostic::new(
                    "design/private-helper-callers",
                    Severity::Info,
                    "private helper has one caller",
                    format!("internal/store/file{}.go", i % 7),
                    span(i as u32 * 5 + 2),
                    Fingerprint::of("design/private-helper-callers", &symbol, ""),
                );
                d.evidence =
                    json!({ "caller": "store::run#function", "callers": 1, "statements": 3 });
                d.symbol = Some(symbol);
                d
            }
            _ => {
                let symbol = format!("store::hub{i}#function");
                let mut d = Diagnostic::new(
                    "design/coupling",
                    Severity::Warn,
                    "function coordinates 14 collaborators in its package",
                    format!("internal/store/hub{}.go", i % 3),
                    span(i as u32 * 7 + 3),
                    Fingerprint::of("design/coupling", &symbol, ""),
                );
                d.evidence = json!({ "fan_in": 2, "fan_out": 14 + i % 3 });
                d.symbol = Some(symbol);
                d
            }
        })
        .collect()
}

fn go_facts(findings: &[Diagnostic]) -> BTreeMap<Fingerprint, Value> {
    findings
        .iter()
        .map(|d| {
            let facts = json!({ "language": "go", "kind": "function", "visibility": "public" });
            (d.fingerprint.clone(), facts)
        })
        .collect()
}

fn gap() -> Vec<Incomplete> {
    vec![Incomplete {
        path: Some("broken.go".into()),
        reason: "type error".to_owned(),
    }]
}

#[test]
fn full_agent_text_gives_each_finding_everything_needed_to_act() {
    let findings = [coupling(), helper(), unknown_rule()];
    let facts = facts(&findings);
    let briefing = Briefing {
        catalog: Some(Catalog::bundled()),
        facts: Some(&facts),
        suppressed: 2,
        allowed: 1,
        detail: Detail::Full,
        ..Briefing::default()
    };
    insta::assert_snapshot!(render_with(Format::Agent, &findings, &gap(), &briefing));
}

#[test]
fn full_agent_json_is_one_tagged_record_per_line() {
    let findings = [coupling(), helper(), unknown_rule()];
    let facts = facts(&findings);
    let briefing = Briefing {
        catalog: Some(Catalog::bundled()),
        facts: Some(&facts),
        suppressed: 2,
        allowed: 1,
        detail: Detail::Full,
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
    assert_eq!(coupling["rule"], "design/coupling");
    assert_eq!(coupling["authored"], "warn");
    assert_eq!(coupling["symbol"], "jit::compile#function");
    assert_eq!(coupling["location"]["line"], 350);
    assert_eq!(coupling["evidence"]["fan_out"], 14);
    assert!(coupling["requirement"].as_str().unwrap().contains("fan"));
    assert!(
        coupling["resolve"].is_object(),
        "a heuristic finding asks for review whatever its severity"
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
    assert!(summary["judgments"]["fail"].is_string());
    assert!(summary["judgments"]["pass"].is_string());
    assert!(summary["judgments"]["notApplicable"].is_string());
}

#[test]
fn agent_formats_state_a_clean_run() {
    let text = render(Format::Agent, &[], &[]);
    assert_eq!(
        text,
        "summary: 0 error, 0 warn, 0 info, 0 review, 0 incomplete, 0 suppressed, 0 allowed\n"
    );
    let json: Value = serde_json::from_str(render(Format::AgentJson, &[], &[]).trim()).unwrap();
    assert_eq!(
        json,
        json!({ "status": "clean", "counts": { "error": 0, "warn": 0, "review": 0 }, "groups": [] })
    );
    let full = Briefing {
        detail: Detail::Full,
        ..Briefing::default()
    };
    let json = render_with(Format::AgentJson, &[], &[], &full);
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
    let excerpt = record["groups"][0]["expected"].as_str().unwrap();
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
fn full_limit_keeps_the_most_severe_findings_and_counts_the_rest() {
    let findings = [helper(), coupling(), unknown_rule()];
    let facts = facts(&findings);
    let briefing = briefed(&facts, |b| Briefing {
        limit: Some(2),
        detail: Detail::Full,
        ..b
    });
    let text = render_with(Format::Agent, &findings, &[], &briefing);
    assert!(text.contains("acme/custom  error"), "{text}");
    assert!(text.contains("design/coupling  warn"), "{text}");
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
        text.contains("judgments:"),
        "a shown heuristic finding asks for review: {text}"
    );

    let json = records_of(&render_with(Format::AgentJson, &findings, &[], &briefing));
    let kinds: Vec<_> = json.iter().map(|r| r["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["finding", "finding", "truncated", "summary"]);
    assert_eq!(json[2]["omitted"], 1);
    assert_eq!(json[0]["rule"], "design/coupling", "original order is kept");
    assert!(json[3]["judgments"].is_object());
}

#[test]
fn limit_counts_findings_and_keeps_errors_then_the_larger_groups() {
    let findings = [helper(), coupling(), unknown_rule()];
    let facts = facts(&findings);
    let briefing = briefed(&facts, |b| Briefing {
        limit: Some(2),
        ..b
    });
    let text = render_with(Format::Agent, &findings, &[], &briefing);
    assert!(text.starts_with("acme/custom error\n"), "{text}");
    assert!(text.contains("design/coupling warn [review]"), "{text}");
    assert!(!text.contains("private-helper-callers"), "{text}");
    assert!(
        text.contains("... 1 more finding(s) not shown (raise --limit)"),
        "{text}"
    );
    assert!(
        text.contains("summary: 1 error, 1 warn, 1 info, 2 review"),
        "{text}"
    );
    assert!(text.contains("judgments:"), "{text}");

    let json = records_of(&render_with(Format::AgentJson, &findings, &[], &briefing));
    assert_eq!(json.len(), 1, "the compact JSON is one object");
    let rules: Vec<_> = json[0]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["rule"].as_str().unwrap())
        .collect();
    assert_eq!(rules, ["acme/custom", "design/coupling"]);
    assert_eq!(json[0]["omitted"], json!({ "groups": 1, "findings": 1 }));
    assert_eq!(json[0]["counts"]["info"], 1, "counts cover the whole run");
    assert!(json[0]["judgments"].is_object());
}

#[test]
fn a_limit_inside_a_group_cuts_it_and_leaves_no_group_out() {
    let findings = flagged(5);
    let briefing = Briefing {
        limit: Some(3),
        ..Briefing::default()
    };
    let out = records_of(&render_with(Format::AgentJson, &findings, &[], &briefing));
    let files = out[0]["groups"][0]["files"].as_object().unwrap();
    assert_eq!(
        files
            .values()
            .map(|rows| rows.as_array().unwrap().len())
            .sum::<usize>(),
        3
    );
    assert_eq!(out[0]["omitted"], json!({ "groups": 0, "findings": 2 }));
}

#[test]
fn notes_say_why_a_finding_is_reported() {
    let findings = [unknown_rule()];
    let facts = facts(&findings);
    let notes = BTreeMap::from([(
        findings[0].fingerprint.clone(),
        "judgment expired: decision changed".to_owned(),
    )]);
    let briefing = briefed(&facts, |b| Briefing {
        notes: Some(&notes),
        ..b
    });
    let text = render_with(Format::Agent, &findings, &[], &briefing);
    assert!(
        text.contains("(note: judgment expired: decision changed)"),
        "{text}"
    );
    let json = records_of(&render_with(Format::AgentJson, &findings, &[], &briefing));
    let instance = &json[0]["groups"][0]["files"]["a.txt"][0];
    assert_eq!(instance[3]["note"], "judgment expired: decision changed");

    let full = Briefing {
        detail: Detail::Full,
        ..briefing
    };
    let json = records_of(&render_with(Format::AgentJson, &findings, &[], &full));
    assert_eq!(json[0]["note"], "judgment expired: decision changed");
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
    let briefing = briefed(&facts, |b| Briefing {
        detail: Detail::Full,
        ..b
    });
    let json = records_of(&render_with(Format::AgentJson, &findings, &[], &briefing));
    let evidence = &json[0]["evidence"];
    assert!(evidence.get("symbol").is_none());
    assert_eq!(evidence["fan_in"], 2);
    assert!(evidence["callees"].as_str().unwrap().len() < 140);
    let text = render_with(Format::Agent, &findings, &[], &briefing);
    assert!(!text.contains("symbol=jit"), "{text}");

    let compact = records_of(&render_with(
        Format::AgentJson,
        &findings,
        &[],
        &briefed(&facts, |b| b),
    ));
    let shared = &compact[0]["groups"][0]["evidence"];
    assert_eq!(shared["fan_in"], 2);
    assert!(shared["callees"].as_str().unwrap().len() < 140);
    assert_eq!(
        shared["symbol"], "jit::compile#function",
        "the message does not name the symbol"
    );
}

#[test]
fn fingerprint_prefixes_are_git_short_and_grow_until_the_findings_are_distinct() {
    let make = |raw: &str| {
        let mut d = unknown_rule();
        d.fingerprint = Fingerprint::from_raw(raw);
        d
    };
    let a = format!("{}1{}", "a".repeat(12), "0".repeat(51));
    let b = format!("{}2{}", "a".repeat(12), "0".repeat(51));
    let c = format!("{}{}", "b".repeat(4), "0".repeat(60));
    let findings = [make(&a), make(&b), make(&c)];
    let text = render(Format::Agent, &findings, &[]);
    for fingerprint in [&a[..13], &b[..13], &c[..7]] {
        assert!(text.contains(&format!(" {fingerprint}\n")), "{text}");
    }
    let full = Briefing {
        detail: Detail::Full,
        ..Briefing::default()
    };
    let text = render_with(Format::Agent, &findings, &[], &full);
    assert!(
        text.contains(&format!("fingerprint: {}", &a[..13])),
        "{text}"
    );
}

fn decision_with(examples: &str) -> Catalog {
    let indented: String = examples.lines().map(|l| format!("  {l}\n")).collect();
    let empty = if examples.is_empty() { " []" } else { "" };
    let decision = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: p/a\n  labels:\n    lighthouse/pack: p\n    lighthouse/section: s\nspec:\n  title: A\n  context: i\n  scope: {{ subject: symbol }}\n  requirement: A MUST b.\n  severity: info\n  check:\n    type: model\n  examples:{empty}\n{indented}"
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
        detail: Detail::Full,
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
fn expected_structure_prefers_canonical_then_a_match_then_the_shortest() {
    let canonical = decision_with(&format!(
        "{}{}",
        example("short-valid", "valid", false, "x"),
        example("long-valid", "valid", true, "x\n          y\n          z")
    ));
    assert_eq!(
        expected_basis(&canonical, "function"),
        Some(("canonical".to_owned(), "long-valid".to_owned()))
    );

    let matched = decision_with(&format!(
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
    ));
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

    assert_eq!(expected_basis(&decision_with(""), "function"), None);
}

#[test]
fn agent_report_has_the_fields_the_json_prints() {
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
    let fields = &report.fields;
    assert_eq!(fields["status"], "incomplete");
    assert_eq!(fields["groups"].as_array().unwrap().len(), 1);
    assert_eq!(fields["omitted"], json!({ "groups": 1, "findings": 1 }));
    assert_eq!(fields["incomplete"], json!([[null, "plugin crashed"]]));
    assert_eq!(fields["counts"]["review"], 1, "counts cover the whole run");
    assert!(
        fields.get("judgments").is_none() && fields.get("resolve").is_none(),
        "the finding that asks for review was left out"
    );

    let all = lighthouse_report::agent_report(
        &findings,
        &[],
        &Briefing {
            catalog: Some(catalog),
            ..Briefing::default()
        },
    );
    assert_eq!(all.fields["status"], "findings");
    assert_eq!(all.fields["groups"].as_array().unwrap().len(), 2);
    assert!(all.fields.get("omitted").is_none());
    assert!(all.fields["judgments"].is_object());
    assert!(all.fields["resolve"].is_string());

    let full = lighthouse_report::agent_report(
        &findings,
        &gaps,
        &Briefing {
            catalog: Some(catalog),
            limit: Some(1),
            detail: Detail::Full,
            ..Briefing::default()
        },
    );
    assert_eq!(full.fields["findings"][0]["type"], "finding");
    assert_eq!(full.fields["omitted"], 1);
    assert_eq!(full.fields["incomplete"][0]["reason"], "plugin crashed");
}

#[test]
fn needs_review_follows_the_authored_severity_of_the_decision_and_the_judgments_that_stand() {
    let briefing = Briefing {
        catalog: Some(Catalog::bundled()),
        ..Briefing::default()
    };
    let mut heuristic = coupling();
    heuristic.severity = Severity::Error;
    assert!(
        briefing.needs_review(&heuristic),
        "a warn decision at error"
    );
    let mechanical = Diagnostic::new(
        "core/allow-reason",
        Severity::Warn,
        "needs a reason",
        "src/lib.rs",
        span(1),
        Fingerprint::of("core/allow-reason", "m", ""),
    );
    assert!(
        !briefing.needs_review(&mechanical),
        "an error decision at warn"
    );
}

#[test]
fn compact_agent_text_groups_findings_by_decision() {
    let mut findings = vec![coupling(), helper(), unknown_rule()];
    findings.extend([exported(1, "bar/bar.go"), exported(2, "bar/bar.go")]);
    findings[4].evidence = json!({ "symbol": "pkg::Func2#function", "receiver": "T" });
    let mut facts = go_facts(&findings);
    facts.extend(self::facts(&findings[..3]));
    let notes = BTreeMap::from([(
        findings[4].fingerprint.clone(),
        "judgment expired: decision changed".to_owned(),
    )]);
    let briefing = Briefing {
        catalog: Some(Catalog::bundled()),
        facts: Some(&facts),
        notes: Some(&notes),
        suppressed: 2,
        allowed: 1,
        ..Briefing::default()
    };
    insta::assert_snapshot!(render_with(Format::Agent, &findings, &gap(), &briefing));
}

#[test]
fn compact_agent_json_groups_findings_by_decision() {
    let mut findings = vec![coupling(), helper(), unknown_rule()];
    findings.extend([exported(1, "bar/bar.go"), exported(2, "bar/bar.go")]);
    findings[4].evidence = json!({ "symbol": "pkg::Func2#function", "receiver": "T" });
    let mut facts = go_facts(&findings);
    facts.extend(self::facts(&findings[..3]));
    let notes = BTreeMap::from([(
        findings[4].fingerprint.clone(),
        "judgment expired: decision changed".to_owned(),
    )]);
    let briefing = Briefing {
        catalog: Some(Catalog::bundled()),
        facts: Some(&facts),
        notes: Some(&notes),
        suppressed: 2,
        allowed: 1,
        ..Briefing::default()
    };
    let out = render_with(Format::AgentJson, &findings, &gap(), &briefing);
    assert_eq!(out.lines().count(), 1, "one object on one line");
    insta::assert_snapshot!(out);
}

#[test]
fn each_further_identical_finding_adds_one_short_line_and_one_entry() {
    let render_n = |n: usize| {
        let findings = flagged(n);
        let facts = go_facts(&findings);
        let briefing = briefed(&facts, |b| b);
        let text = render_with(Format::Agent, &findings, &[], &briefing);
        let json = records_of(&render_with(Format::AgentJson, &findings, &[], &briefing));
        (text, json)
    };
    let (small, small_json) = render_n(5);
    let (large, large_json) = render_n(6);
    assert_eq!(large.lines().count(), small.lines().count() + 1);
    let added = large
        .lines()
        .find(|line| !small.lines().any(|l| l == *line))
        .unwrap();
    assert!(added.len() < 100, "{added}");
    assert_eq!(large.matches("design/exported-doc").count(), 1, "{large}");
    assert_eq!(large.matches("expected:").count(), 1, "{large}");
    let entries = |json: &[Value]| -> usize {
        json[0]["groups"][0]["files"]
            .as_object()
            .unwrap()
            .values()
            .map(|rows| rows.as_array().unwrap().len())
            .sum()
    };
    assert_eq!(entries(&large_json), entries(&small_json) + 1);
    let row = large_json[0]["groups"][0]["files"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|rows| rows.as_array().unwrap())
        .next()
        .unwrap();
    assert_eq!(row.as_array().unwrap().len(), 3, "{row}");
}

#[test]
fn compact_output_is_at_most_forty_percent_of_the_full_output() {
    let findings = mixed(50);
    let facts = go_facts(&findings);
    let bytes = |format: Format, detail: Detail| {
        let briefing = briefed(&facts, |b| Briefing { detail, ..b });
        render_with(format, &findings, &[], &briefing).len()
    };
    for format in [Format::Agent, Format::AgentJson] {
        let compact = bytes(format, Detail::Compact);
        let full = bytes(format, Detail::Full);
        assert!(
            compact * 100 <= full * 40,
            "{format:?}: compact {compact} bytes vs full {full} bytes"
        );
    }
}

#[test]
fn format_and_detail_parse_by_name() {
    assert_eq!("compact".parse::<Detail>().unwrap(), Detail::Compact);
    assert_eq!("full".parse::<Detail>().unwrap(), Detail::Full);
    assert!("verbose".parse::<Detail>().is_err());
    assert_eq!(Detail::default(), Detail::Compact);
}

#[test]
fn shown_is_what_the_limit_keeps() {
    let findings = [helper(), coupling(), unknown_rule()];
    let briefing = Briefing {
        limit: Some(2),
        ..Briefing::default()
    };
    let kept: Vec<_> = shown(&findings, &briefing)
        .into_iter()
        .map(|d| d.rule_id.as_str())
        .collect();
    assert_eq!(kept.len(), 2);
    assert!(
        kept.contains(&"acme/custom"),
        "errors are kept first: {kept:?}"
    );
}

#[test]
fn briefing_group_options_carry_the_limit_and_the_catalog() {
    let briefing = Briefing {
        catalog: Some(Catalog::bundled()),
        limit: Some(4),
        mcp: true,
        ..Briefing::default()
    };
    let options = briefing.group_options();
    assert_eq!(options.limit, Some(4));
    assert!(options.catalog.is_some() && options.mcp);
}
