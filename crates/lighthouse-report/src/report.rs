use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    path::PathBuf,
};

use lighthouse_model::{
    Diagnostic, Fingerprint, Incomplete, Preview, Severity, Suppressed, squash,
};
use lighthouse_spec::{Catalog, authored_severity};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    descriptor::Descriptor,
    expected::{ExampleKey, Expected, expected},
    group::{
        Entry, GroupOptions, Grouped, Groups, Judgments, judgments, judgments_line, resolve_command,
    },
    text::{evidence, evidence_line},
};

/// What the agent formats add to a bare diagnostic: the catalog the rules
/// come from, what the analysis knew about each finding, how many findings
/// judgments and source directives kept out of the report, and how many to
/// print.
#[derive(Debug, Default, Clone, Copy)]
pub struct Briefing<'a> {
    /// The decisions findings cite. Whether a finding asks for review is
    /// the authored severity of its decision; without the catalog a finding is
    /// told by its severity alone, so every frontend that counts reviews
    /// passes it.
    pub catalog: Option<&'a Catalog>,
    /// Subject facts by fingerprint; the `language`, `kind` and `visibility`
    /// entries pick the expected structure.
    pub facts: Option<&'a BTreeMap<Fingerprint, Value>>,
    /// Why a finding is reported although a judgment was recorded on it, by
    /// fingerprint.
    pub notes: Option<&'a BTreeMap<Fingerprint, String>>,
    /// The findings a judgment stands for and that are reported: they do not
    /// ask for review.
    pub judged: Option<&'a BTreeSet<Fingerprint>>,
    /// Findings judgments kept out of the report.
    pub suppressed: usize,
    /// Findings source directives suppress.
    pub allowed: usize,
    /// Every suppressed finding with its suppression: SARIF lists them.
    pub suppressions: Option<&'a [Suppressed]>,
    /// Print at most this many findings, errors first, and say how many were
    /// left out.
    pub limit: Option<usize>,
    pub detail: Detail,
    /// The verified previews of a fix run, by fingerprint: the SARIF `fixes`
    /// of the findings they are for, each relative to the original text. A
    /// check computes none.
    pub fixes: Option<&'a BTreeMap<String, Preview>>,
    /// The text of the files of findings, by project-relative path, for
    /// formats whose columns count characters and not bytes (SARIF).
    pub sources: Option<&'a BTreeMap<String, String>>,
    /// The report is data for a program: hints name tool calls
    /// (`fix {...}`), not command lines.
    pub tool_form: bool,
    /// How the check and the fix of each decision run, by decision id.
    pub descriptors: Option<&'a BTreeMap<String, Descriptor>>,
    /// What the run adds to SARIF's property bag of the run, such as what the
    /// agents of a fix run did.
    pub properties: Option<&'a serde_json::Map<String, Value>>,
}

/// How much each finding carries.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// Findings grouped by decision, then by file: what is common to a
    /// decision is said once and a finding is one short line.
    #[default]
    Compact,
    /// One self-contained record per finding.
    Full,
}

impl Briefing<'_> {
    /// How the compact shape is cut.
    pub fn group_options(&self) -> GroupOptions<'_> {
        GroupOptions {
            catalog: self.catalog,
            limit: self.limit,
            tool_form: self.tool_form,
            descriptors: self.descriptors,
        }
    }

    /// Whether the finding is a review task: its decision authored `warn` or
    /// `info`, whatever severity the configuration reports it at, and no
    /// judgment stands for it. An authored `error` is definitive. A rule
    /// without a decision is told by its severity.
    pub fn needs_review(&self, diagnostic: &Diagnostic) -> bool {
        let decision = self
            .catalog
            .and_then(|catalog| catalog.decision(&diagnostic.rule_id));
        if diagnostic.review.is_some() {
            return true;
        }
        let judged = self
            .judged
            .is_some_and(|judged| judged.contains(&diagnostic.fingerprint));
        authored_severity(diagnostic.severity, decision).needs_review(judged)
    }
}

/// The JSON report of a check as typed data, which a frontend prints or
/// wraps: the findings grouped by decision, or one self-contained record per
/// finding.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Report {
    Compact(Compact),
    Full(Full),
}

/// How a compact report stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Clean,
    /// Some finding is a decision's verdict.
    Findings,
    /// Every finding is a subject nothing decided, left to review.
    Review,
    /// Part of the project was not checked.
    Incomplete,
}

/// The counts of a compact report: findings by severity (a subject left to
/// review is no finding) and those that ask for review, always; the rest when
/// they are not zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Counts {
    pub error: usize,
    pub warn: usize,
    pub review: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<usize>,
    /// Findings judgments kept out of the report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suppressed: Option<usize>,
    /// Findings source directives suppress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed: Option<usize>,
}

/// The report with the findings grouped by decision.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Compact {
    pub status: Status,
    pub counts: Counts,
    #[serde(flatten)]
    pub groups: Groups,
    /// What was not checked: `[path, reason]` pairs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub incomplete: Vec<(Option<PathBuf>, String)>,
}

/// The report with one record per finding.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Full {
    pub findings: Vec<Value>,
    pub incomplete: Vec<Gap>,
    /// Findings the limit left out.
    pub omitted: usize,
    /// What each judgment says; present when a shown finding asks for review.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judgments: Option<Judgments>,
}

/// A part of the project that was not checked, and why.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Gap {
    /// `incomplete`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub path: Option<PathBuf>,
    pub reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    path: String,
    line: u32,
    column: u32,
    end_line: u32,
}

/// How a judgment is recorded on a finding.
#[derive(Serialize)]
struct Resolve {
    command: String,
}

#[derive(Serialize)]
struct Finding<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    decision: &'a str,
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
    /// The model that decided the finding, with its version; evidence of
    /// nothing, so apart from it.
    #[serde(rename = "modelVersion", skip_serializing_if = "Option::is_none")]
    model_version: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<Expected>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    fingerprint: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolve: Option<Resolve>,
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

/// Builds the [`Report`]: the JSON report of a check.
pub fn report(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> Report {
    match briefing.detail {
        Detail::Compact => Report::Compact(compact(diagnostics, incomplete, briefing)),
        Detail::Full => Report::Full(full(diagnostics, incomplete, briefing)),
    }
}

/// The findings to print: all of them, or the first `limit` by severity
/// (errors first) in their original order, and how many were left out.
pub(crate) fn select(
    diagnostics: &[Diagnostic],
    limit: Option<usize>,
) -> (Vec<&Diagnostic>, usize) {
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

/// The line that states one gap of the analysis.
pub(crate) fn incomplete_line(item: &Incomplete) -> String {
    match &item.path {
        Some(path) => format!("{}: incomplete: {}", path.display(), item.reason),
        None => format!("incomplete: {}", item.reason),
    }
}

/// One header per decision with its requirement and expected structure, one
/// line per finding under it, then the gaps, how to record a judgment and the
/// judgments it accepts when a shown finding asks for review, and the summary.
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
        let _ = writeln!(out, "resolve: {}", resolve_command("<fingerprint>"));
        let _ = writeln!(out, "judgments: {}", judgments_line());
    }
    let _ = writeln!(out, "{}", summary_line(diagnostics, incomplete, briefing));
    out
}

/// The diagnostics as entries of the compact shape.
fn entries(diagnostics: &[Diagnostic], briefing: &Briefing<'_>) -> Vec<Entry> {
    diagnostics
        .iter()
        .map(|d| {
            let decision = briefing.catalog.and_then(|c| c.decision(&d.rule_id));
            Entry {
                decision: d.rule_id.clone(),
                severity: d.severity,
                authored: authored_severity(d.severity, decision),
                review: briefing.needs_review(d),
                path: d.file.display().to_string(),
                line: d.span.start.line,
                col: d.span.start.col,
                message: d.message.clone(),
                symbol: d.symbol.clone(),
                evidence: d.evidence.clone(),
                attributes: d
                    .model_version
                    .iter()
                    .map(|model| ("modelVersion".to_owned(), json!(model)))
                    .collect(),
                note: note_of(d, briefing),
                fingerprint: d.fingerprint.as_str().to_owned(),
                facts: briefing.facts.and_then(|f| f.get(&d.fingerprint)).cloned(),
            }
        })
        .collect()
}

/// Why the finding is reported although a judgment was recorded on it: its
/// own review note, else the briefing's.
fn note_of(diagnostic: &Diagnostic, briefing: &Briefing) -> Option<String> {
    diagnostic.review.clone().or_else(|| {
        briefing
            .notes
            .and_then(|notes| notes.get(&diagnostic.fingerprint))
            .cloned()
    })
}

/// One block per finding, then the gaps, a footer naming the judgments review
/// accepts when a finding asks for review, and the summary.
fn full_text(diagnostics: &[Diagnostic], incomplete: &[Incomplete], briefing: &Briefing) -> String {
    let (shown, omitted) = select(diagnostics, briefing.limit);
    let widths = widths(diagnostics);
    let mut out = String::new();
    for diagnostic in &shown {
        let width = widths[diagnostic.fingerprint.as_str()];
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
    if shown.iter().any(|d| briefing.needs_review(d)) {
        let _ = writeln!(out, "judgments: {}", judgments_line());
    }
    let _ = writeln!(out, "{}", summary_line(diagnostics, incomplete, briefing));
    out
}

/// The compact report: the grouped findings with the status and counts of the
/// whole run, and the gaps.
fn compact(diagnostics: &[Diagnostic], incomplete: &[Incomplete], briefing: &Briefing) -> Compact {
    let grouped = Grouped::of(entries(diagnostics, briefing), &briefing.group_options());
    let status = if !incomplete.is_empty() {
        Status::Incomplete
    } else if diagnostics.is_empty() {
        Status::Clean
    } else if diagnostics.iter().all(|d| d.review.is_some()) {
        Status::Review
    } else {
        Status::Findings
    };
    let count = |s: Severity| severity_count(diagnostics, s);
    let some = |n: usize| (n > 0).then_some(n);
    Compact {
        status,
        counts: Counts {
            error: count(Severity::Error),
            warn: count(Severity::Warn),
            review: diagnostics
                .iter()
                .filter(|d| briefing.needs_review(d))
                .count(),
            info: some(count(Severity::Info)),
            suppressed: some(briefing.suppressed),
            allowed: some(briefing.allowed),
        },
        groups: grouped.data(),
        incomplete: incomplete
            .iter()
            .map(|item| (item.path.clone(), item.reason.clone()))
            .collect(),
    }
}

/// The full report: the findings that fit `briefing.limit`, the gaps, how many
/// findings were left out, and the judgments table when a shown finding asks
/// for review.
fn full(diagnostics: &[Diagnostic], incomplete: &[Incomplete], briefing: &Briefing) -> Full {
    let (shown, omitted) = select(diagnostics, briefing.limit);
    let widths = widths(diagnostics);
    let findings = shown
        .iter()
        .map(|d| {
            let record = finding(d, briefing, widths[d.fingerprint.as_str()]);
            serde_json::to_value(record).expect("finding serializes")
        })
        .collect();
    Full {
        findings,
        incomplete: incomplete
            .iter()
            .map(|item| Gap {
                kind: "incomplete",
                path: item.path.clone(),
                reason: item.reason.clone(),
            })
            .collect(),
        omitted,
        judgments: shown
            .iter()
            .any(|d| briefing.needs_review(d))
            .then(judgments),
    }
}

/// How many characters of each fingerprint tell it apart from the others of
/// the run, by fingerprint.
fn widths(diagnostics: &[Diagnostic]) -> BTreeMap<&str, usize> {
    let fingerprints: Vec<&str> = diagnostics.iter().map(|d| d.fingerprint.as_str()).collect();
    let widths = Fingerprint::prefix_widths(&fingerprints);
    fingerprints.into_iter().zip(widths).collect()
}

fn finding<'a>(diagnostic: &'a Diagnostic, briefing: &Briefing, width: usize) -> Finding<'a> {
    let decision = briefing
        .catalog
        .and_then(|catalog| catalog.decision(&diagnostic.rule_id));
    let facts = briefing
        .facts
        .and_then(|facts| facts.get(&diagnostic.fingerprint));
    let key = ExampleKey::new(facts);
    let fingerprint = diagnostic.fingerprint.as_str();
    Finding {
        kind: if diagnostic.review.is_some() {
            "review"
        } else {
            "finding"
        },
        decision: &diagnostic.rule_id,
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
        requirement: decision.map(|d| squash(&d.requirement)),
        context: decision.map(|d| squash(&d.context)),
        evidence: evidence(&diagnostic.evidence, diagnostic.symbol.as_deref()),
        model_version: diagnostic.model_version.as_deref(),
        expected: decision.and_then(|d| expected(d, &key, &diagnostic.file)),
        note: note_of(diagnostic, briefing),
        fingerprint,
        resolve: briefing.needs_review(diagnostic).then(|| Resolve {
            command: resolve_command(prefix(fingerprint, width)),
        }),
    }
}

fn prefix(fingerprint: &str, width: usize) -> &str {
    fingerprint.get(..width).unwrap_or(fingerprint)
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
        finding.decision, finding.severity, location.path, location.line, location.column
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
        let lines: Vec<String> = std::iter::once(expected.heading()).chain(picture).collect();
        fields.push(("expected:", lines.join("\n")));
    }
    fields.extend(finding.note.clone().map(|text| ("note:", text)));
    fields.push((
        "fingerprint:",
        prefix(finding.fingerprint, width).to_owned(),
    ));
    if let Some(resolve) = &finding.resolve {
        fields.push(("resolve:", resolve.command.clone()));
    }
    fields
}

/// How many findings have this severity; a subject left to review is no
/// finding.
fn severity_count(diagnostics: &[Diagnostic], severity: Severity) -> usize {
    diagnostics
        .iter()
        .filter(|d| d.review.is_none() && d.severity == severity)
        .count()
}

fn summary_line(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    let count = |s: Severity| severity_count(diagnostics, s);
    let reviews = diagnostics
        .iter()
        .filter(|d| briefing.needs_review(d))
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
