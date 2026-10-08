use std::{collections::BTreeMap, fmt::Write, path::Path};

use lighthouse_model::{Diagnostic, Fingerprint, Incomplete, Severity, Verdict};
use lighthouse_spec::{Catalog, Decision, Example, ExampleKind, tier};
use serde::Serialize;
use serde_json::{Map, Value, json};

/// Longest excerpt of an example, in lines and in characters.
const EXCERPT_LINES: usize = 12;
const EXCERPT_CHARS: usize = 600;
/// Longest tuning text and longest single evidence value, in characters.
const TUNING_CHARS: usize = 300;
const VALUE_CHARS: usize = 120;
/// Shortest fingerprint prefix shown; longer when needed to stay unambiguous.
const PREFIX_MIN: usize = 12;

/// What the agent formats add to a bare diagnostic: the catalog the rules
/// come from, what the analysis knew about each finding, how many findings
/// verdicts and source annotations kept out of the report, and how many to
/// print.
#[derive(Debug, Default, Clone, Copy)]
pub struct Briefing<'a> {
    /// The decisions findings cite. Whether a finding asks for a verdict is
    /// the tier of its decision; without the catalog a finding is judged by
    /// its severity alone, so every frontend that counts reviews passes it.
    pub catalog: Option<&'a Catalog>,
    /// Subject facts by fingerprint; the `language`, `kind` and `visibility`
    /// entries pick the expected structure.
    pub facts: Option<&'a BTreeMap<Fingerprint, Value>>,
    /// Why a finding is reported although a verdict was recorded on it, by
    /// fingerprint.
    pub notes: Option<&'a BTreeMap<Fingerprint, String>>,
    pub suppressed: usize,
    /// Findings allowed by source annotations.
    pub allowed: usize,
    /// Print at most this many findings, errors first, and say how many were
    /// left out.
    pub limit: Option<usize>,
}

impl Briefing<'_> {
    /// Whether the finding is a review task: its decision is enforced by a
    /// heuristic or a judgment, whatever the severity. A rule without a
    /// decision is judged by its severity.
    pub fn needs_verdict(&self, diagnostic: &Diagnostic) -> bool {
        let decision = self
            .catalog
            .and_then(|catalog| catalog.decision(&diagnostic.rule_id));
        lighthouse_spec::needs_verdict(tier(diagnostic.severity, decision))
    }
}

/// The agent records as typed data: the findings that fit `briefing.limit`,
/// the gaps, how many findings were left out, and the reasons table when a
/// shown finding asks for a verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentReport {
    pub findings: Vec<Value>,
    pub incomplete: Vec<Value>,
    pub omitted: usize,
    pub reasons: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    path: String,
    line: u32,
    column: u32,
    end_line: u32,
}

/// A short picture of what the code should look like.
#[derive(Serialize)]
struct Expected {
    /// `example` (a valid example of the decision) or `tuning` (its note for
    /// the language).
    source: &'static str,
    language: String,
    /// Why this one: `canonical`, `matches kind=function`, `shortest valid
    /// example` or `tuning`.
    basis: String,
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
    evidence: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<Expected>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    fingerprint: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolve: Option<Value>,
}

/// What the expected structure is chosen by: the file's language and the kind
/// and visibility of the finding's symbol.
struct Subject<'a> {
    language: Option<&'a str>,
    kind: Option<&'a str>,
    visibility: Option<&'a str>,
}

impl<'a> Subject<'a> {
    fn of(facts: Option<&'a Value>) -> Self {
        let text = |key: &str| facts.and_then(|f| f.get(key)).and_then(Value::as_str);
        Self {
            language: text("language"),
            kind: text("kind"),
            visibility: text("visibility"),
        }
    }
}

/// One block per finding, then the gaps, a footer naming the reasons review
/// verdicts accept when a finding asks for review, and the summary.
pub(crate) fn text(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    let (shown, omitted) = select(diagnostics, briefing.limit);
    let width = prefix_len(&shown);
    let mut out = String::new();
    for diagnostic in &shown {
        out.push_str(&block(&finding(diagnostic, briefing, width), width));
        out.push('\n');
    }
    if omitted > 0 {
        let _ = writeln!(
            out,
            "... {omitted} more finding(s) not shown (raise --limit)"
        );
    }
    for item in incomplete {
        let _ = writeln!(out, "{}", incomplete_line(item));
    }
    if shown.iter().any(|d| briefing.needs_verdict(d)) {
        let _ = writeln!(out, "reasons: {}", reasons_line());
    }
    let _ = writeln!(out, "{}", summary_line(diagnostics, incomplete, briefing));
    out
}

/// Builds the [`AgentReport`] that [`json_lines`] prints.
pub fn agent_report(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> AgentReport {
    let (shown, omitted) = select(diagnostics, briefing.limit);
    let width = prefix_len(&shown);
    let findings = shown
        .iter()
        .map(|d| serde_json::to_value(finding(d, briefing, width)).expect("finding serializes"))
        .collect();
    let incomplete = incomplete
        .iter()
        .map(|item| json!({ "type": "incomplete", "path": item.path, "reason": item.reason }))
        .collect();
    AgentReport {
        findings,
        incomplete,
        omitted,
        reasons: shown
            .iter()
            .any(|d| briefing.needs_verdict(d))
            .then(reasons),
    }
}

/// One JSON object per line: findings, incomplete entries, a `truncated`
/// record when `limit` left findings out, then the summary, each tagged with
/// `type`. The summary carries the reasons table once, when a shown finding
/// asks for review.
pub(crate) fn json_lines(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    let report = agent_report(diagnostics, incomplete, briefing);
    let mut lines: Vec<String> = report
        .findings
        .iter()
        .chain(&report.incomplete)
        .map(Value::to_string)
        .collect();
    if report.omitted > 0 {
        lines.push(json!({ "type": "truncated", "omitted": report.omitted }).to_string());
    }
    let count = |s: Severity| diagnostics.iter().filter(|d| d.severity == s).count();
    let mut summary = json!({
        "type": "summary",
        "errors": count(Severity::Error),
        "warnings": count(Severity::Warn),
        "infos": count(Severity::Info),
        "reviews": diagnostics.iter().filter(|d| briefing.needs_verdict(d)).count(),
        "incomplete": incomplete.len(),
        "suppressed": briefing.suppressed,
        "allowed": briefing.allowed,
    });
    if let Some(reasons) = report.reasons {
        summary["reasons"] = reasons;
    }
    lines.push(summary.to_string());
    lines.iter().map(|line| format!("{line}\n")).collect()
}

/// The findings to print: all of them, or the first `limit` by severity
/// (errors first) in their original order, and how many were left out.
fn select(diagnostics: &[Diagnostic], limit: Option<usize>) -> (Vec<&Diagnostic>, usize) {
    let Some(limit) = limit.filter(|&l| l < diagnostics.len()) else {
        return (diagnostics.iter().collect(), 0);
    };
    let mut order: Vec<usize> = (0..diagnostics.len()).collect();
    order.sort_by_key(|&i| diagnostics[i].severity);
    order.truncate(limit);
    order.sort_unstable();
    let shown = order.into_iter().map(|i| &diagnostics[i]).collect();
    (shown, diagnostics.len() - limit)
}

/// How many characters of a fingerprint tell the shown findings apart.
fn prefix_len(shown: &[&Diagnostic]) -> usize {
    let distinct = |len: usize| {
        let mut seen = std::collections::BTreeSet::new();
        shown
            .iter()
            .all(|d| seen.insert(prefix(d.fingerprint.as_str(), len)))
    };
    (PREFIX_MIN..=64).find(|&len| distinct(len)).unwrap_or(64)
}

fn prefix(fingerprint: &str, len: usize) -> &str {
    fingerprint.get(..len).unwrap_or(fingerprint)
}

fn finding<'a>(diagnostic: &'a Diagnostic, briefing: &Briefing, width: usize) -> Finding<'a> {
    let decision = briefing
        .catalog
        .and_then(|catalog| catalog.decision(&diagnostic.rule_id));
    let facts = briefing
        .facts
        .and_then(|facts| facts.get(&diagnostic.fingerprint));
    let subject = Subject::of(facts);
    Finding {
        kind: "finding",
        rule: &diagnostic.rule_id,
        severity: diagnostic.severity,
        tier: tier(diagnostic.severity, decision),
        location: Location {
            path: diagnostic.file.display().to_string(),
            line: diagnostic.span.start.line,
            column: diagnostic.span.start.col,
            end_line: diagnostic.span.end.line,
        },
        symbol: diagnostic.symbol.as_deref(),
        message: &diagnostic.message,
        requirement: decision.map(|d| one_line(&d.requirement)),
        intent: decision.map(|d| one_line(&d.intent)),
        evidence: evidence(&diagnostic.evidence, diagnostic.symbol.as_deref()),
        expected: decision.and_then(|d| expected(d, &subject, &diagnostic.file)),
        note: briefing
            .notes
            .and_then(|notes| notes.get(&diagnostic.fingerprint))
            .cloned(),
        fingerprint: diagnostic.fingerprint.as_str(),
        resolve: briefing
            .needs_verdict(diagnostic)
            .then(|| resolve(diagnostic, width)),
    }
}

/// The canonical valid example for the file's language; else the valid
/// example whose name (or whose invalid counterpart's) mentions the kind or
/// visibility of the finding's symbol; else the shortest valid example; else
/// the decision's tuning note for the language. Each kept short.
fn expected(decision: &Decision, subject: &Subject, file: &Path) -> Option<Expected> {
    let valid: Vec<&Example> = decision
        .examples
        .iter()
        .filter(|e| e.kind == ExampleKind::Valid)
        .filter(|e| subject.language.is_none_or(|l| e.language == l))
        .collect();
    let chosen = valid
        .iter()
        .find(|e| e.canonical)
        .map(|e| (*e, "canonical".to_owned()))
        .or_else(|| matching(decision, &valid, subject))
        .or_else(|| {
            let shortest = valid.iter().min_by_key(|e| size(e))?;
            Some((*shortest, "shortest valid example".to_owned()))
        });
    if let Some((example, basis)) = chosen {
        return Some(from_example(example, basis, file));
    }
    let language = subject.language?;
    let text = decision.languages.get(language)?.tuning.as_ref()?;
    Some(Expected {
        source: "tuning",
        language: language.to_owned(),
        basis: "tuning".to_owned(),
        name: None,
        path: None,
        excerpt: truncate(&one_line(text), TUNING_CHARS),
    })
}

/// The valid example that mentions the most of the subject's kind and
/// visibility, in its own name or in its invalid counterpart's; none when no
/// example mentions either.
fn matching<'a>(
    decision: &'a Decision,
    valid: &[&'a Example],
    subject: &Subject,
) -> Option<(&'a Example, String)> {
    let score = |example: &Example| {
        let counterpart = decision
            .examples
            .iter()
            .filter(|e| e.kind == ExampleKind::Invalid && e.language == example.language)
            .find(|e| pair_key(&e.name) == pair_key(&example.name))
            .map_or("", |e| e.name.as_str());
        let names = format!("{} {counterpart}", example.name).to_lowercase();
        [subject.kind, subject.visibility]
            .into_iter()
            .flatten()
            .filter(|word| names.contains(*word))
            .count()
    };
    let best = valid
        .iter()
        .map(|e| (score(e), *e))
        .filter(|(score, _)| *score > 0)
        .max_by_key(|(score, _)| *score)?
        .1;
    let kind = subject.kind.unwrap_or("symbol");
    Some((best, format!("matches kind={kind}")))
}

/// A name without the words that mark an example valid or invalid, so
/// `rust-valid` and `rust-invalid` pair up.
fn pair_key(name: &str) -> String {
    name.replace("invalid", "").replace("valid", "")
}

fn size(example: &Example) -> usize {
    example.files.iter().map(|f| f.text().len()).sum()
}

fn from_example(example: &Example, basis: String, file: &Path) -> Expected {
    let extension = file.extension();
    let chosen = example
        .files
        .iter()
        .find(|f| Path::new(&f.path).extension() == extension)
        .or(example.files.first());
    Expected {
        source: "example",
        language: example.language.clone(),
        basis,
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

/// The evidence without values that repeat the owner symbol, each value cut
/// to a bounded length.
fn evidence(evidence: &Value, symbol: Option<&str>) -> Value {
    let Some(map) = evidence.as_object() else {
        return evidence.clone();
    };
    let kept: Map<String, Value> = map
        .iter()
        .filter(|(_, value)| value.as_str().is_none_or(|s| Some(s) != symbol))
        .map(|(key, value)| (key.clone(), bounded(value)))
        .collect();
    if kept.is_empty() {
        Value::Null
    } else {
        Value::Object(kept)
    }
}

fn bounded(value: &Value) -> Value {
    let text = value.to_string();
    if text.chars().count() > VALUE_CHARS {
        Value::String(truncate(&text, VALUE_CHARS))
    } else {
        value.clone()
    }
}

fn resolve(diagnostic: &Diagnostic, width: usize) -> Value {
    json!({
        "command": format!(
            "lighthouse review resolve {} --verdict <verdict> --reason <reason> --reviewer-kind agent",
            prefix(diagnostic.fingerprint.as_str(), width)
        ),
    })
}

fn reasons() -> Value {
    let verdicts: BTreeMap<&str, Vec<&str>> =
        [Verdict::Confirmed, Verdict::Rejected, Verdict::Deferred]
            .into_iter()
            .map(|v| (v.as_str(), v.reasons().iter().map(|r| r.as_str()).collect()))
            .collect();
    json!(verdicts)
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

fn block(finding: &Finding, width: usize) -> String {
    let location = &finding.location;
    let mut out = format!(
        "{}  {} ({})  {}:{}:{}\n",
        finding.rule, finding.severity, finding.tier, location.path, location.line, location.column
    );
    for (label, value) in fields(finding, width) {
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
fn fields(finding: &Finding, width: usize) -> Vec<(&'static str, String)> {
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
    fields.extend(evidence_line(&finding.evidence).map(|text| ("evidence:", text)));
    if let Some(expected) = &finding.expected {
        let picture = expected.excerpt.lines().map(|l| format!("  {l}"));
        let lines: Vec<String> = std::iter::once(expected_heading(expected))
            .chain(picture)
            .collect();
        fields.push(("expected:", lines.join("\n")));
    }
    fields.extend(finding.note.clone().map(|text| ("note:", text)));
    fields.push((
        "fingerprint:",
        prefix(finding.fingerprint, width).to_owned(),
    ));
    if let Some(resolve) = &finding.resolve {
        let command = resolve["command"].as_str().unwrap_or_default();
        fields.push(("resolve:", command.to_owned()));
    }
    fields
}

fn expected_heading(expected: &Expected) -> String {
    match (&expected.name, &expected.path) {
        (Some(name), Some(path)) => format!(
            "valid {} example `{name}` ({path}), {}",
            expected.language, expected.basis
        ),
        (Some(name), None) => format!(
            "valid {} example `{name}`, {}",
            expected.language, expected.basis
        ),
        _ => format!("{} tuning", expected.language),
    }
}

/// `key=value` pairs of an evidence object.
fn evidence_line(evidence: &Value) -> Option<String> {
    let pairs: Vec<String> = evidence
        .as_object()?
        .iter()
        .map(|(key, value)| format!("{key}={}", evidence_value(value)))
        .collect();
    (!pairs.is_empty()).then(|| pairs.join(" "))
}

fn evidence_value(value: &Value) -> String {
    match value {
        Value::String(s) if !s.contains(char::is_whitespace) && !s.is_empty() => s.clone(),
        other => other.to_string(),
    }
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
    let reviews = diagnostics
        .iter()
        .filter(|d| briefing.needs_verdict(d))
        .count();
    format!(
        "summary: {} error, {} warn, {} info, {} review, {} incomplete, {} suppressed, {} allowed",
        count(Severity::Error),
        count(Severity::Warn),
        count(Severity::Info),
        reviews,
        incomplete.len(),
        briefing.suppressed,
        briefing.allowed
    )
}
