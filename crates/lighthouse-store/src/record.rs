use lighthouse_model::{Diagnostic, Label, Reason, ReviewerKind, Severity, Verdict};
use serde::Serialize;
use serde_json::{Value, json};

/// One finding as a run saw it. The locator and evidence are JSON so that
/// artifacts other than source code fit the same record.
#[derive(Debug, Clone, PartialEq)]
pub struct Observed {
    pub fingerprint: String,
    pub rule_id: String,
    pub severity: Severity,
    /// The artifact, project-relative with `/` separators.
    pub path: String,
    /// Where in the artifact: `{"span": {"start": {"line", "col"}, "end": ...}}`
    /// for text.
    pub locator: Value,
    pub symbol: Option<String>,
    pub message: String,
    pub evidence: Value,
    /// What the analysis knew about the subject: language, symbol shape,
    /// measures. Frozen into a review's feature snapshot.
    pub facts: Value,
}

impl Observed {
    /// The record of a diagnostic, with the facts the analysis gathered for it.
    pub fn from_diagnostic(diagnostic: &Diagnostic, facts: Value) -> Self {
        Self {
            fingerprint: diagnostic.fingerprint.as_str().to_owned(),
            rule_id: diagnostic.rule_id.clone(),
            severity: diagnostic.severity,
            path: diagnostic.file.to_string_lossy().replace('\\', "/"),
            locator: json!({ "span": diagnostic.span }),
            symbol: diagnostic.symbol.clone(),
            message: diagnostic.message.clone(),
            evidence: diagnostic.evidence.clone(),
            facts,
        }
    }
}

/// What one analysis run saw and what it covered. Findings absent from
/// `observed` are resolved only inside `reported` and `rules`, and only when
/// the analysis was `complete`.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// Every finding of the report scope, suppressed ones included.
    pub observed: Vec<Observed>,
    /// Project-relative paths whose findings the run reports: a file or a
    /// directory prefix; the empty path is everything.
    pub reported: Vec<String>,
    /// The rules that ran.
    pub rules: Vec<String>,
    /// Nothing in the analysis scope was left unanalyzed.
    pub complete: bool,
}

/// What recording a run changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunSummary {
    pub opened: usize,
    pub reopened: usize,
    pub resolved: usize,
}

/// Which findings a listing includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter {
    /// Still reported and not suppressed.
    Open,
    /// Suppressed by their latest verdict.
    Suppressed,
    All,
}

/// What a listing selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub rule: Option<String>,
    pub status: StatusFilter,
}

/// The most recent verdict on a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LatestReview {
    pub verdict: Verdict,
    pub reason: Reason,
}

/// A finding as stored: its latest sighting, its history and its standing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindingRecord {
    pub fingerprint: String,
    pub rule_id: String,
    pub severity: String,
    pub path: String,
    pub locator: Value,
    pub symbol: Option<String>,
    pub first_seen: String,
    pub last_seen: String,
    /// Set when a complete run no longer reported the finding.
    pub resolved_at: Option<String>,
    /// How many times it came back after being resolved.
    pub reopened: u32,
    pub message: String,
    pub evidence: Value,
    pub facts: Value,
    /// The latest verdict, if the finding was reviewed.
    pub review: Option<LatestReview>,
    /// The latest verdict is a rejection.
    pub suppressed: bool,
}

/// A verdict to record. `fingerprint` may be an unambiguous prefix.
#[derive(Debug, Clone, PartialEq)]
pub struct NewReview {
    pub fingerprint: String,
    pub verdict: Verdict,
    pub reason: Reason,
    pub reason_text: Option<String>,
    pub reviewer_kind: ReviewerKind,
    pub reviewer_id: Option<String>,
    /// Hash of the pattern the rule implements, when it has one.
    pub rule_version: Option<String>,
    pub catalog_version: Option<String>,
    /// Identity of the code pattern the finding belongs to; later phases fill it.
    pub pattern_fingerprint: Option<String>,
    /// The pattern's scope (`symbol`, `file`, ...).
    pub scope: Option<String>,
    /// The commit the reviewed code was at, when known.
    pub commit: Option<String>,
}

/// One row of the append-only review log.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewEvent {
    pub id: i64,
    pub fingerprint: String,
    pub rule_id: String,
    pub rule_version: Option<String>,
    pub catalog_version: Option<String>,
    pub pattern_fingerprint: Option<String>,
    pub verdict: Verdict,
    pub reason: Reason,
    pub reason_text: Option<String>,
    pub reviewer_kind: ReviewerKind,
    pub reviewer_id: Option<String>,
    pub language: Option<String>,
    pub scope: Option<String>,
    pub evidence: Value,
    pub feature_snapshot: Value,
    pub commit: Option<String>,
    pub timestamp: String,
}

impl ReviewEvent {
    /// What the event teaches a model about its rule.
    pub fn label(&self) -> Label {
        Label::of(self.verdict, self.reason)
    }
}
