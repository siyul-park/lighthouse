use std::{collections::BTreeMap, fmt::Write, path::Path};

use lighthouse_model::{Diagnostic, Fingerprint, Incomplete, Severity, Verdict};
use lighthouse_spec::{Catalog, Enforcement, Example, Kind, Pattern};
use serde::Serialize;
use serde_json::{Value, json};

/// Longest excerpt of an example, in lines and in characters.
const EXCERPT_LINES: usize = 12;
const EXCERPT_CHARS: usize = 600;
/// Longest tuning text and longest single evidence value, in characters.
const TUNING_CHARS: usize = 300;
const VALUE_CHARS: usize = 120;

/// What the agent formats add to a bare diagnostic: the catalog the rules
/// come from, what the analysis knew about each finding, and how many
/// findings review verdicts kept out of the report.
#[derive(Debug, Default, Clone, Copy)]
pub struct Briefing<'a> {
    pub catalog: Option<&'a Catalog>,
    /// Subject facts by fingerprint; the `language` entry picks the
    /// language-specific expected structure.
    pub facts: Option<&'a BTreeMap<Fingerprint, Value>>,
    pub suppressed: usize,
}

#[derive(Serialize)]
struct Location {
    path: String,
    line: u32,
    column: u32,
    end_line: u32,
}

/// A short picture of what the code should look like.
#[derive(Serialize)]
struct Expected {
    /// `example` (a valid example of the pattern) or `tuning` (its note for
    /// the language).
    source: &'static str,
    language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    excerpt: String,
}

#[derive(Serialize)]
struct Finding<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    rule: &'a str,
    severity: Severity,
    tier: &'static str,
    location: Location,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<&'a str>,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    requirement: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    intent: Option<String>,
    #[serde(skip_serializing_if = "Value::is_null")]
    evidence: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<Expected>,
    fingerprint: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolve: Option<Value>,
}

/// One block per finding, then the gaps, a footer naming the reasons review
/// verdicts accept when a finding asks for review, and the summary.
pub(crate) fn text(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    let mut out = String::new();
    for diagnostic in diagnostics {
        out.push_str(&block(&finding(diagnostic, briefing)));
        out.push('\n');
    }
    for item in incomplete {
        let _ = writeln!(out, "{}", incomplete_line(item));
    }
    if diagnostics.iter().any(|d| d.severity == Severity::Review) {
        let _ = writeln!(out, "reasons: {}", reasons_line());
    }
    let _ = writeln!(out, "{}", summary_line(diagnostics, incomplete, briefing));
    out
}

/// One JSON object per line: findings, incomplete entries, then the summary,
/// each tagged with `type`.
pub(crate) fn json_lines(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    let mut lines: Vec<String> = diagnostics
        .iter()
        .map(|d| serde_json::to_string(&finding(d, briefing)).expect("finding serializes"))
        .collect();
    lines.extend(incomplete.iter().map(|item| {
        json!({ "type": "incomplete", "path": item.path, "reason": item.reason }).to_string()
    }));
    let count = |s: Severity| diagnostics.iter().filter(|d| d.severity == s).count();
    lines.push(
        json!({
            "type": "summary",
            "errors": count(Severity::Error),
            "warnings": count(Severity::Warn),
            "reviews": count(Severity::Review),
            "incomplete": incomplete.len(),
            "suppressed": briefing.suppressed,
        })
        .to_string(),
    );
    lines.iter().map(|line| format!("{line}\n")).collect()
}

fn finding<'a>(diagnostic: &'a Diagnostic, briefing: &Briefing) -> Finding<'a> {
    let pattern = briefing
        .catalog
        .and_then(|catalog| catalog.pattern(&diagnostic.rule_id));
    let language = briefing
        .facts
        .and_then(|facts| facts.get(&diagnostic.fingerprint))
        .and_then(|facts| facts.get("language"))
        .and_then(Value::as_str);
    Finding {
        kind: "finding",
        rule: &diagnostic.rule_id,
        severity: diagnostic.severity,
        tier: tier(diagnostic.severity, pattern),
        location: Location {
            path: diagnostic.file.display().to_string(),
            line: diagnostic.span.start.line,
            column: diagnostic.span.start.col,
            end_line: diagnostic.span.end.line,
        },
        symbol: diagnostic.symbol.as_deref(),
        message: &diagnostic.message,
        requirement: pattern.map(|p| one_line(&p.requirement)),
        intent: pattern.map(|p| one_line(&p.intent)),
        evidence: &diagnostic.evidence,
        expected: pattern.and_then(|p| expected(p, language, &diagnostic.file)),
        fingerprint: diagnostic.fingerprint.as_str(),
        resolve: (diagnostic.severity == Severity::Review).then(|| resolve(diagnostic)),
    }
}

/// How the finding is decided: the pattern's enforcement, or, for a rule
/// without a pattern, what its severity implies.
fn tier(severity: Severity, pattern: Option<&Pattern>) -> &'static str {
    match pattern.map(|p| p.enforcement) {
        Some(Enforcement::Mechanical) => "mechanical",
        Some(Enforcement::Heuristic) => "heuristic",
        Some(Enforcement::Judgment) => "judgment",
        Some(Enforcement::Doc) | None => match severity {
            Severity::Error => "mechanical",
            Severity::Warn => "heuristic",
            Severity::Review => "judgment",
            Severity::Info => "evidence",
        },
    }
}

/// A valid example for the file's language, else the pattern's tuning for it,
/// else a valid example of any language, each kept short.
fn expected(pattern: &Pattern, language: Option<&str>, file: &Path) -> Option<Expected> {
    let valid = |wanted: Option<&str>| {
        pattern
            .examples
            .iter()
            .find(|e| e.kind == Kind::Valid && wanted.is_none_or(|l| e.language == l))
    };
    let tuning = language.and_then(|l| pattern.tuning.get(l).map(|text| (l, text)));
    match (language.and_then(|l| valid(Some(l))), tuning, valid(None)) {
        (Some(example), _, _) | (None, None, Some(example)) => Some(from_example(example, file)),
        (None, Some((language, text)), _) => Some(Expected {
            source: "tuning",
            language: language.to_owned(),
            name: None,
            path: None,
            excerpt: truncate(&one_line(text), TUNING_CHARS),
        }),
        (None, None, None) => None,
    }
}

fn from_example(example: &Example, file: &Path) -> Expected {
    let extension = file.extension();
    let chosen = example
        .files
        .iter()
        .find(|f| Path::new(&f.path).extension() == extension)
        .or(example.files.first());
    Expected {
        source: "example",
        language: example.language.clone(),
        name: Some(example.name.clone()),
        path: chosen.map(|f| f.path.clone()),
        excerpt: chosen.map(|f| excerpt(f.text())).unwrap_or_default(),
    }
}

fn excerpt(text: &str) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let kept = lines[..lines.len().min(EXCERPT_LINES)].join("\n");
    let cut = truncate(&kept, EXCERPT_CHARS);
    if lines.len() > EXCERPT_LINES && cut == kept {
        format!("{cut}\n...")
    } else {
        cut
    }
}

fn truncate(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_owned(),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn resolve(diagnostic: &Diagnostic) -> Value {
    let verdicts: BTreeMap<&str, Vec<&str>> =
        [Verdict::Confirmed, Verdict::Rejected, Verdict::Deferred]
            .into_iter()
            .map(|v| (v.as_str(), v.reasons().iter().map(|r| r.as_str()).collect()))
            .collect();
    json!({
        "command": format!(
            "lighthouse review resolve {} --verdict <verdict> --reason <reason> --reviewer-kind agent",
            diagnostic.fingerprint.as_str()
        ),
        "verdicts": verdicts,
    })
}

fn reasons_line() -> String {
    [Verdict::Confirmed, Verdict::Rejected, Verdict::Deferred]
        .into_iter()
        .map(|v| {
            let reasons: Vec<&str> = v.reasons().iter().map(|r| r.as_str()).collect();
            format!("{v}={}", reasons.join("|"))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn block(finding: &Finding) -> String {
    let location = &finding.location;
    let mut out = format!(
        "{}  {} ({})  {}:{}:{}\n",
        finding.rule, finding.severity, finding.tier, location.path, location.line, location.column
    );
    for (label, value) in fields(finding) {
        let mut lines = value.lines();
        let first = format!("  {label:<13}{}", lines.next().unwrap_or_default());
        let _ = writeln!(out, "{}", first.trim_end());
        for line in lines {
            let _ = writeln!(out, "{}", format!("               {line}").trim_end());
        }
    }
    out
}

/// The labeled lines of a finding's block, in reading order; a value may span
/// several lines.
fn fields(finding: &Finding) -> Vec<(&'static str, String)> {
    let mut fields = Vec::new();
    if let Some(symbol) = finding.symbol {
        fields.push(("owner:", symbol.to_owned()));
    }
    fields.push(("message:", finding.message.to_owned()));
    fields.extend(
        finding
            .requirement
            .clone()
            .map(|text| ("requirement:", text)),
    );
    fields.extend(finding.intent.clone().map(|text| ("intent:", text)));
    fields.extend(evidence_line(finding.evidence).map(|text| ("evidence:", text)));
    if let Some(expected) = &finding.expected {
        let heading = expected_heading(expected);
        let picture = expected.excerpt.lines().map(|l| format!("  {l}"));
        let lines: Vec<String> = std::iter::once(heading).chain(picture).collect();
        fields.push(("expected:", lines.join("\n")));
    }
    fields.push(("fingerprint:", finding.fingerprint.to_owned()));
    if let Some(resolve) = &finding.resolve {
        let command = resolve["command"].as_str().unwrap_or_default();
        fields.push(("resolve:", command.to_owned()));
    }
    fields
}

fn expected_heading(expected: &Expected) -> String {
    match (&expected.name, &expected.path) {
        (Some(name), Some(path)) => {
            format!("valid {} example `{name}` ({path})", expected.language)
        }
        (Some(name), None) => format!("valid {} example `{name}`", expected.language),
        _ => format!("{} tuning", expected.language),
    }
}

/// `key=value` pairs of an evidence object; long values are cut.
fn evidence_line(evidence: &Value) -> Option<String> {
    let pairs: Vec<String> = evidence
        .as_object()?
        .iter()
        .map(|(key, value)| format!("{key}={}", evidence_value(value)))
        .collect();
    (!pairs.is_empty()).then(|| pairs.join(" "))
}

fn evidence_value(value: &Value) -> String {
    let text = match value {
        Value::String(s) if !s.contains(char::is_whitespace) && !s.is_empty() => s.clone(),
        other => other.to_string(),
    };
    truncate(&text, VALUE_CHARS)
}

fn incomplete_line(item: &Incomplete) -> String {
    match &item.path {
        Some(path) => format!("{}: incomplete: {}", path.display(), item.reason),
        None => format!("incomplete: {}", item.reason),
    }
}

fn summary_line(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    let count = |s: Severity| diagnostics.iter().filter(|d| d.severity == s).count();
    format!(
        "summary: {} error, {} warn, {} review, {} incomplete, {} suppressed",
        count(Severity::Error),
        count(Severity::Warn),
        count(Severity::Review),
        incomplete.len(),
        briefing.suppressed
    )
}
