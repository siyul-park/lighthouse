use std::{collections::BTreeMap, fmt::Write, str::FromStr};

use lighthouse_model::{Diagnostic, Fingerprint, Incomplete, Severity};
use lighthouse_spec::{Catalog, authored_severity};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::{
    evidence::{evidence, evidence_line},
    expected::{Expected, Subject, expected, one_line},
    fix::{ProposedFix, Shown},
    group::{Entry, GroupOptions, Grouped, reasons, reasons_line},
};

/// Shortest fingerprint prefix the full shape shows; longer when needed to
/// stay unambiguous.
const PREFIX_MIN: usize = 12;

/// What the agent formats add to a bare diagnostic: the catalog the rules
/// come from, what the analysis knew about each finding, how many findings
/// verdicts and source annotations kept out of the report, and how many to
/// print.
#[derive(Debug, Default, Clone, Copy)]
pub struct Briefing<'a> {
    /// The decisions findings cite. Whether a finding asks for a verdict is
    /// the authored severity of its decision; without the catalog a finding is judged by
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
    pub detail: Detail,
    /// The fixes proposed for findings, by fingerprint: shown with the
    /// finding, and as `fixes` in SARIF.
    pub fixes: Option<&'a BTreeMap<String, ProposedFix>>,
    /// The text of the files of findings, by project-relative path, for
    /// formats whose columns count characters and not bytes (SARIF).
    pub sources: Option<&'a BTreeMap<String, String>>,
    /// The report is read through MCP, so fixing is the `fix` tool.
    pub mcp: bool,
}

/// How much each finding carries.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Detail {
    /// Findings grouped by decision, then by file: what is common to a
    /// decision is said once and a finding is one short line.
    #[default]
    Compact,
    /// One self-contained record per finding.
    Full,
}

/// The text given to [`Detail::from_str`] names no level of detail.
#[derive(Debug, Error)]
#[error("unknown detail `{0}` (expected compact or full)")]
pub struct UnknownDetail(String);

impl FromStr for Detail {
    type Err = UnknownDetail;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "compact" => Ok(Self::Compact),
            "full" => Ok(Self::Full),
            _ => Err(UnknownDetail(s.to_owned())),
        }
    }
}

impl Briefing<'_> {
    /// How the compact shape is cut.
    pub fn group_options(&self) -> GroupOptions<'_> {
        GroupOptions {
            catalog: self.catalog,
            limit: self.limit,
            mcp: self.mcp,
        }
    }

    /// Whether the finding is a review task: its decision authored `warn` or
    /// `info`, whatever severity the configuration reports it at. An authored
    /// `error` is definitive and needs no verdict. A rule without a decision
    /// is judged by its severity.
    pub fn needs_verdict(&self, diagnostic: &Diagnostic) -> bool {
        let decision = self
            .catalog
            .and_then(|catalog| catalog.decision(&diagnostic.rule_id));
        authored_severity(diagnostic.severity, decision).needs_verdict()
    }
}

/// The agent report as typed data, the fields of the JSON object a frontend
/// prints or wraps.
///
/// Compact: `status`, `counts`, `groups`, then when they apply `incomplete`
/// (`[path, reason]` pairs), `omitted`, and the `resolve` hint and `reasons`
/// table once for all shown findings that ask for a verdict. Full: `findings`,
/// `incomplete` and `reasons` as records, `omitted` as a count.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentReport {
    pub fields: Map<String, Value>,
}

/// The full shape: the findings that fit `briefing.limit`, the gaps, how many
/// findings were left out, and the reasons table when a shown finding asks
/// for a verdict.
struct FullReport {
    findings: Vec<Value>,
    incomplete: Vec<Value>,
    omitted: usize,
    reasons: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    path: String,
    line: u32,
    column: u32,
    end_line: u32,
}

#[derive(Serialize)]
struct Finding<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    rule: &'a str,
    severity: Severity,
    authored: Severity,
    location: Location,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<&'a str>,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    requirement: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
    #[serde(skip_serializing_if = "Value::is_null")]
    evidence: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<Expected>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    fingerprint: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolve: Option<Value>,
}

/// The agent text: grouped by decision unless `briefing.detail` is full.
pub(crate) fn text(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    match briefing.detail {
        Detail::Compact => compact_text(diagnostics, incomplete, briefing),
        Detail::Full => full_text(diagnostics, incomplete, briefing),
    }
}

/// The findings the agent formats show under `briefing.limit`: what a fix
/// preview is worth computing for.
pub fn shown<'d>(diagnostics: &'d [Diagnostic], briefing: &Briefing) -> Vec<&'d Diagnostic> {
    if briefing.detail == Detail::Full {
        return select(diagnostics, briefing.limit).0;
    }
    let grouped = Grouped::of(entries(diagnostics, briefing), &briefing.group_options());
    let kept = grouped.fingerprints();
    diagnostics
        .iter()
        .filter(|d| kept.contains(d.fingerprint.as_str()))
        .collect()
}

/// Builds the [`AgentReport`] that [`json_lines`] prints.
pub fn agent_report(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> AgentReport {
    let fields = match briefing.detail {
        Detail::Compact => compact_fields(diagnostics, incomplete, briefing),
        Detail::Full => {
            let full = full_report(diagnostics, incomplete, briefing);
            let mut fields = Map::new();
            fields.insert("findings".to_owned(), Value::Array(full.findings));
            fields.insert("incomplete".to_owned(), Value::Array(full.incomplete));
            fields.insert("omitted".to_owned(), json!(full.omitted));
            if let Some(reasons) = full.reasons {
                fields.insert("reasons".to_owned(), reasons);
            }
            fields
        }
    };
    AgentReport { fields }
}

/// The agent JSON: one object on one line in the compact shape; with full
/// detail one object per line, see [`full_json_lines`].
pub(crate) fn json_lines(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    match briefing.detail {
        Detail::Compact => {
            let fields = compact_fields(diagnostics, incomplete, briefing);
            format!("{}\n", Value::Object(fields))
        }
        Detail::Full => full_json_lines(diagnostics, incomplete, briefing),
    }
}

/// One header per decision with its requirement and expected structure, one
/// line per finding under it, then the gaps, how to record a verdict and the
/// reasons it accepts when a shown finding asks for review, and the summary.
fn compact_text(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    let grouped = Grouped::of(entries(diagnostics, briefing), &briefing.group_options());
    let mut out = grouped.text();
    for item in incomplete {
        let _ = writeln!(out, "{}", incomplete_line(item));
    }
    if grouped.asks_review() {
        let _ = writeln!(
            out,
            "resolve: lighthouse review resolve <fingerprint> --verdict <verdict> --reason <reason> --reviewer-kind agent"
        );
        let _ = writeln!(out, "reasons: {}", reasons_line());
    }
    let _ = writeln!(out, "{}", summary_line(diagnostics, incomplete, briefing));
    out
}

/// The diagnostics as entries of the compact shape.
fn entries<'a>(diagnostics: &[Diagnostic], briefing: &Briefing<'a>) -> Vec<Entry<'a>> {
    diagnostics
        .iter()
        .map(|d| {
            let decision = briefing.catalog.and_then(|c| c.decision(&d.rule_id));
            Entry {
                rule: d.rule_id.clone(),
                severity: d.severity,
                authored: authored_severity(d.severity, decision),
                review: briefing.needs_verdict(d),
                path: d.file.display().to_string(),
                line: d.span.start.line,
                col: d.span.start.col,
                message: d.message.clone(),
                symbol: d.symbol.clone(),
                evidence: d.evidence.clone(),
                attributes: Map::new(),
                note: briefing.notes.and_then(|n| n.get(&d.fingerprint)).cloned(),
                fingerprint: d.fingerprint.as_str().to_owned(),
                facts: briefing.facts.and_then(|f| f.get(&d.fingerprint)).cloned(),
                fix: briefing.fixes.and_then(|f| f.get(d.fingerprint.as_str())),
            }
        })
        .collect()
}

/// One block per finding, then the gaps, a footer naming the reasons review
/// verdicts accept when a finding asks for review, and the summary.
fn full_text(diagnostics: &[Diagnostic], incomplete: &[Incomplete], briefing: &Briefing) -> String {
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

/// The compact fields: the grouped findings with the status and counts of the
/// whole run, and the gaps.
fn compact_fields(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> Map<String, Value> {
    let grouped = Grouped::of(entries(diagnostics, briefing), &briefing.group_options());
    let mut fields = grouped.fields();
    let status = if !incomplete.is_empty() {
        "incomplete"
    } else if diagnostics.is_empty() {
        "clean"
    } else {
        "findings"
    };
    fields.insert("status".to_owned(), json!(status));
    fields.insert("counts".to_owned(), counts(diagnostics, briefing));
    if !incomplete.is_empty() {
        let gaps: Vec<Value> = incomplete
            .iter()
            .map(|item| json!([item.path, item.reason]))
            .collect();
        fields.insert("incomplete".to_owned(), Value::Array(gaps));
    }
    fields
}

/// Errors, warnings and findings asking for a verdict always; the other
/// counts when they are not zero.
fn counts(diagnostics: &[Diagnostic], briefing: &Briefing) -> Value {
    let count = |s: Severity| diagnostics.iter().filter(|d| d.severity == s).count();
    let reviews = diagnostics
        .iter()
        .filter(|d| briefing.needs_verdict(d))
        .count();
    let mut counts = Map::new();
    counts.insert("error".to_owned(), json!(count(Severity::Error)));
    counts.insert("warn".to_owned(), json!(count(Severity::Warn)));
    counts.insert("review".to_owned(), json!(reviews));
    for (key, n) in [
        ("info", count(Severity::Info)),
        ("suppressed", briefing.suppressed),
        ("allowed", briefing.allowed),
    ] {
        if n > 0 {
            counts.insert(key.to_owned(), json!(n));
        }
    }
    Value::Object(counts)
}

fn full_report(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> FullReport {
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
    FullReport {
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
fn full_json_lines(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    let report = full_report(diagnostics, incomplete, briefing);
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
        authored: authored_severity(diagnostic.severity, decision),
        location: Location {
            path: diagnostic.file.display().to_string(),
            line: diagnostic.span.start.line,
            column: diagnostic.span.start.col,
            end_line: diagnostic.span.end.line,
        },
        symbol: diagnostic.symbol.as_deref(),
        message: &diagnostic.message,
        requirement: decision.map(|d| one_line(&d.requirement)),
        context: decision.map(|d| one_line(&d.context)),
        evidence: evidence(&diagnostic.evidence, diagnostic.symbol.as_deref()),
        expected: decision.and_then(|d| expected(d, &subject, &diagnostic.file)),
        note: briefing
            .notes
            .and_then(|notes| notes.get(&diagnostic.fingerprint))
            .cloned(),
        fingerprint: diagnostic.fingerprint.as_str(),
        fix: briefing
            .fixes
            .and_then(|f| f.get(diagnostic.fingerprint.as_str()))
            .map(|fix| Shown::of(fix, &diagnostic.file.display().to_string()).json()),
        resolve: briefing
            .needs_verdict(diagnostic)
            .then(|| resolve(diagnostic, width)),
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

fn block(finding: &Finding, width: usize) -> String {
    let location = &finding.location;
    let authored = if finding.authored == finding.severity {
        String::new()
    } else {
        format!(" (authored {})", finding.authored)
    };
    let mut out = format!(
        "{}  {}{authored}  {}:{}:{}\n",
        finding.rule, finding.severity, location.path, location.line, location.column
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
    fields.extend(finding.context.clone().map(|text| ("context:", text)));
    fields.extend(evidence_line(&finding.evidence).map(|text| ("evidence:", text)));
    if let Some(expected) = &finding.expected {
        let picture = expected.excerpt.lines().map(|l| format!("  {l}"));
        let lines: Vec<String> = std::iter::once(expected_heading(expected))
            .chain(picture)
            .collect();
        fields.push(("expected:", lines.join("\n")));
    }
    fields.extend(finding.note.clone().map(|text| ("note:", text)));
    if let Some(fix) = &finding.fix {
        let safety = fix["safety"].as_str().unwrap_or_default();
        let body = fix["diff"].as_str().or(fix["summary"].as_str());
        fields.push(("fix:", format!("{safety}\n{}", body.unwrap_or_default())));
    }
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
