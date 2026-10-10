//! The grouped shape of the agent formats, and what a finding's fix looks
//! like in them and in SARIF.

use std::collections::BTreeMap;

use lighthouse_model::{Diagnostic, Fingerprint, Position, Severity, Span};
use lighthouse_report::{
    Briefing, Entry, FixFile, Format, GroupOptions, Grouped, ProposedFix, render_with,
};
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

fn entry(
    rule: &str,
    severity: Severity,
    path: &str,
    line: u32,
    fingerprint: &str,
) -> Entry<'static> {
    Entry {
        rule: rule.to_owned(),
        severity,
        authored: severity,
        review: severity != Severity::Error,
        path: path.to_owned(),
        line,
        col: 1,
        message: format!("finding at {line}"),
        symbol: None,
        evidence: Value::Null,
        attributes: serde_json::Map::new(),
        note: None,
        fingerprint: fingerprint.to_owned(),
        facts: None,
        fix: None,
    }
}

/// A fingerprint that starts with `n`, so short prefixes tell them apart.
fn spread(n: u32) -> String {
    format!("{n:x}{}", "0".repeat(63))
}

fn grouped_entries() -> Vec<Entry<'static>> {
    let fingerprint = |n: u32| spread(n);
    vec![
        entry("acme/a", Severity::Warn, "x.go", 3, &fingerprint(1)),
        entry("acme/a", Severity::Warn, "x.go", 1, &fingerprint(2)),
        entry("acme/b", Severity::Error, "y.go", 9, &fingerprint(3)),
    ]
}

#[test]
fn grouped_of_orders_errors_first_and_cuts_at_the_limit() {
    let all = Grouped::of(grouped_entries(), &GroupOptions::default());
    let rules: Vec<_> = all.fields()["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["rule"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(rules, ["acme/b", "acme/a"]);

    let cut = Grouped::of(
        grouped_entries(),
        &GroupOptions {
            limit: Some(2),
            ..GroupOptions::default()
        },
    );
    assert_eq!(
        cut.fields()["omitted"],
        json!({ "groups": 0, "findings": 1 })
    );
    let rows = &cut.fields()["groups"][1]["files"]["x.go"];
    assert_eq!(rows[0][0], "1:1", "findings are in file order");
}

#[test]
fn grouped_fields_say_how_to_resolve_once_when_a_finding_asks_for_review() {
    let grouped = Grouped::of(grouped_entries(), &GroupOptions::default());
    let fields = grouped.fields();
    assert!(fields["resolve"].is_string() && fields["reasons"].is_object());
    let errors_only = Grouped::of(
        vec![entry("acme/b", Severity::Error, "y.go", 9, &spread(3))],
        &GroupOptions::default(),
    );
    assert!(errors_only.fields().get("resolve").is_none());
}

#[test]
fn grouped_asks_review_when_a_shown_finding_does() {
    assert!(Grouped::of(grouped_entries(), &GroupOptions::default()).asks_review());
    let only_error = vec![entry("acme/b", Severity::Error, "y.go", 9, &spread(3))];
    assert!(!Grouped::of(only_error, &GroupOptions::default()).asks_review());
}

#[test]
fn grouped_fingerprints_are_the_full_ones_of_the_findings_kept() {
    let cut = Grouped::of(
        grouped_entries(),
        &GroupOptions {
            limit: Some(1),
            ..GroupOptions::default()
        },
    );
    let kept: Vec<_> = cut.fingerprints().into_iter().collect();
    assert_eq!(kept, [spread(3)]);
}

#[test]
fn grouped_text_prints_a_header_and_a_line_per_finding() {
    let text = Grouped::of(grouped_entries(), &GroupOptions::default()).text();
    assert!(
        text.starts_with("acme/b error\n  y.go:9:1 finding at 9 3000000\n"),
        "{text}"
    );
    assert!(text.contains("acme/a warn [review]\n  x.go:1:1"), "{text}");
}

#[test]
fn a_mixed_group_asks_for_review_and_marks_the_instances_that_do() {
    let mut asking = entry("acme/a", Severity::Warn, "x.go", 1, &spread(1));
    asking.review = true;
    let mut plain = entry("acme/a", Severity::Warn, "x.go", 2, &spread(2));
    plain.review = false;
    let mut reversed = vec![plain.clone(), asking.clone()];
    reversed.reverse();
    for entries in [vec![asking.clone(), plain.clone()], reversed] {
        let grouped = Grouped::of(entries, &GroupOptions::default());
        assert!(grouped.asks_review());
        let fields = grouped.fields();
        let group = &fields["groups"][0];
        assert_eq!(group["review"], true);
        let rows = group["files"]["x.go"].as_array().unwrap();
        assert_eq!(rows[0][3], json!({ "review": true }));
        assert_eq!(rows[1].as_array().unwrap().len(), 3, "{rows:?}");
        assert!(fields["resolve"].is_string());
        assert!(
            grouped
                .text()
                .contains("x.go:1:1 finding at 1 1000000 [review]\n")
        );
    }
    let only = Grouped::of(vec![asking], &GroupOptions::default());
    let rows = &only.fields()["groups"][0]["files"]["x.go"];
    assert_eq!(rows[0].as_array().unwrap().len(), 3, "not mixed: no marker");
}

#[test]
fn a_symbol_is_dropped_only_when_the_message_names_it_as_a_word() {
    let mut named = entry("acme/a", Severity::Warn, "x.go", 1, &spread(1));
    named.symbol = Some("pkg::Foo#function".to_owned());
    named.message = "function Foo is bad".to_owned();
    let mut confusable = named.clone();
    confusable.message = "function FooBar is bad".to_owned();
    confusable.fingerprint = spread(2);
    confusable.line = 2;
    let fields = Grouped::of(vec![named], &GroupOptions::default()).fields();
    assert!(fields["groups"][0].get("evidence").is_none(), "{fields:?}");
    let fields = Grouped::of(vec![confusable], &GroupOptions::default()).fields();
    assert_eq!(
        fields["groups"][0]["evidence"]["symbol"],
        "pkg::Foo#function"
    );
}

#[test]
fn evidence_note_and_fix_have_their_own_slots() {
    let mut e = entry("acme/a", Severity::Warn, "x.go", 1, &spread(1));
    e.evidence = json!({ "note": "evidence called note", "fix": 1, "other": 1 });
    e.note = Some("the real note".to_owned());
    let mut f = entry("acme/a", Severity::Warn, "x.go", 2, &spread(2));
    f.evidence = json!({ "note": "different", "fix": 2, "other": 1 });
    let fields = Grouped::of(vec![e, f], &GroupOptions::default()).fields();
    let rows = fields["groups"][0]["files"]["x.go"].as_array().unwrap();
    assert_eq!(rows[0][3]["note"], "the real note");
    assert_eq!(rows[0][3]["evidence"]["note"], "evidence called note");
    assert_eq!(fields["groups"][0]["evidence"], json!({ "other": 1 }));
}

fn at(line: u32, col: u32) -> Position {
    Position { line, col }
}

#[test]
fn sarif_columns_count_utf16_code_units_and_not_bytes() {
    // "é" is two bytes and one unit; "😀" four bytes and two units.
    let line = "é😀x := 1";
    let before = format!("{line}\n");
    let mut d = unknown_rule();
    d.file = "a.go".into();
    d.span = Span {
        start: at(1, 7),
        end: at(1, 8),
    };
    let fix = ProposedFix {
        safety: lighthouse_model::Safety::Safe,
        description: "rename".to_owned(),
        files: vec![FixFile::new(
            "a.go".to_owned(),
            &before,
            "é😀y := 1\n",
            [(at(1, 7), at(1, 8), "y".to_owned())],
        )],
    };
    let fixes = BTreeMap::from([(d.fingerprint.as_str().to_owned(), fix)]);
    let sources = BTreeMap::from([("a.go".to_owned(), before)]);
    let briefing = Briefing {
        fixes: Some(&fixes),
        sources: Some(&sources),
        ..Briefing::default()
    };
    let sarif: Value =
        serde_json::from_str(&render_with(Format::Sarif, &[d], &[], &briefing)).unwrap();
    let result = &sarif["runs"][0]["results"][0];
    let region = &result["locations"][0]["physicalLocation"]["region"];
    assert_eq!(
        (region["startColumn"].as_u64(), region["endColumn"].as_u64()),
        (Some(4), Some(5))
    );
    let deleted = &result["fixes"][0]["artifactChanges"][0]["replacements"][0]["deletedRegion"];
    assert_eq!(
        (
            deleted["startColumn"].as_u64(),
            deleted["endColumn"].as_u64()
        ),
        (Some(4), Some(5))
    );
}

#[test]
fn a_fix_diff_names_the_files_when_it_edits_one_besides_the_findings() {
    let file = |path: &str| {
        FixFile::new(
            path.to_owned(),
            "a\nb\n",
            "a\nc\n",
            [(at(2, 1), at(2, 2), "c".to_owned())],
        )
    };
    let fix = |files| ProposedFix {
        safety: lighthouse_model::Safety::Safe,
        description: "edit".to_owned(),
        files,
    };
    let findings = [unknown_rule()];
    let render_fix = |fix: ProposedFix| {
        let fixes = BTreeMap::from([(findings[0].fingerprint.as_str().to_owned(), fix)]);
        let briefing = Briefing {
            fixes: Some(&fixes),
            ..Briefing::default()
        };
        let out = render_with(Format::AgentJson, &findings, &[], &briefing);
        let report: Value = serde_json::from_str(&out).unwrap();
        report["groups"][0]["files"]["a.txt"][0][3]["fix"]["diff"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let own = render_fix(fix(vec![file("a.txt")]));
    assert!(own.starts_with("@@ "), "{own}");
    assert!(own.ends_with("+c"), "no trailing newline: {own:?}");
    let other = render_fix(fix(vec![file("a.txt"), file("b.txt")]));
    assert!(
        other.starts_with("--- a/a.txt\n+++ b/a.txt\n@@ "),
        "{other}"
    );
    assert!(other.contains("--- a/b.txt\n+++ b/b.txt\n"), "{other}");
}
