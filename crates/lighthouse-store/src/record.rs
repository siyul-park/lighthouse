use std::fmt;

use lighthouse_model::{Diagnostic, Label, Reason, ReviewerKind, Severity, Verdict};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::digest;

/// One finding as a run saw it. The locator and evidence are JSON so that
/// artifacts other than source code fit the same record.
#[derive(Debug, Clone, PartialEq)]
pub struct Observed {
    pub fingerprint: String,
    pub rule_id: String,
    pub severity: Severity,
    /// The severity its decision authored (`error`, `warn`, `info`): what
    /// decides whether a verdict may hide the finding. For a rule without a
    /// decision, the severity it reported.
    pub authored_severity: String,
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
    /// The options the rule ran with, defaults included.
    pub options: Value,
    /// Hash of what the rule's decision means; verdicts expire when it moves.
    pub rule_version: Option<String>,
    /// The versions earlier builds recorded verdicts under (comma separated),
    /// for as long as the decision still means the same: a verdict recorded
    /// then does not expire for a change of format or of how it is checked.
    pub legacy_rule_version: Option<String>,
    /// Hash of how the decision is checked. Recorded, never compared to
    /// expire a verdict.
    pub check_revision: Option<String>,
    /// Hash of the whole decision definition, wording and examples included.
    pub decision_hash: Option<String>,
}

impl Observed {
    /// The record of a diagnostic, with the facts the analysis gathered for
    /// it. Authored severity, options and rule versions start unknown; set them directly.
    pub fn from_diagnostic(diagnostic: &Diagnostic, facts: Value) -> Self {
        Self {
            fingerprint: diagnostic.fingerprint.as_str().to_owned(),
            rule_id: diagnostic.rule_id.clone(),
            severity: diagnostic.severity,
            authored_severity: String::new(),
            path: diagnostic.file.to_string_lossy().replace('\\', "/"),
            locator: json!({ "span": diagnostic.span }),
            symbol: diagnostic.symbol.clone(),
            message: diagnostic.message.clone(),
            evidence: diagnostic.evidence.clone(),
            facts,
            options: json!({}),
            rule_version: None,
            legacy_rule_version: None,
            check_revision: None,
            decision_hash: None,
        }
    }

    /// Identifies the evidence for as long as it says the same thing: a hash
    /// of its normalized values.
    pub fn evidence_digest(&self) -> String {
        digest::evidence(&self.evidence)
    }
}

/// What a run could not check, and therefore cannot say was fixed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unchecked {
    Nothing,
    /// Directories (project-relative, `/`-separated) holding files that could
    /// not be analyzed; findings under them are left as they are.
    Paths(Vec<String>),
    /// A gap that cannot be placed, such as a language provider that failed.
    Everything,
}

/// What one analysis run saw and what it covered. Findings absent from
/// `observed` are resolved only inside `reported` and `rules`, and not where
/// the run `unchecked` something.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// Every finding of the report scope, suppressed ones included.
    pub observed: Vec<Observed>,
    /// Project-relative paths whose findings the run reports: a file or a
    /// directory prefix; the empty path is everything.
    pub reported: Vec<String>,
    /// The rules that ran.
    pub rules: Vec<String>,
    /// Every rule the configuration enables; remembered findings of any other
    /// rule become inactive.
    pub configured: Vec<String>,
    pub unchecked: Unchecked,
    /// The commit and whether the working tree differed from it.
    pub commit: Option<String>,
    pub dirty: bool,
    pub catalog_version: Option<String>,
    pub lighthouse_version: String,
}

/// What recording a run changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunSummary {
    pub opened: usize,
    pub reopened: usize,
    pub resolved: usize,
    pub deactivated: usize,
}

/// Which findings a listing includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter {
    /// Still reported: not resolved, inactive or suppressed.
    Open,
    /// Kept out of reports by their latest verdict.
    Suppressed,
    /// Suppressed with `scope-too-broad`: the rule should be narrowed.
    Narrowing,
    /// Of a rule the configuration no longer enables.
    Inactive,
    /// No longer reported by a complete run.
    Resolved,
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

/// What the latest rejection does to a finding now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Standing {
    /// The finding stays out of reports.
    Suppressed,
    /// The rule's requirement changed since the verdict: ask again.
    RuleChanged,
    /// The finding's evidence changed since the verdict: ask again.
    EvidenceChanged,
    /// Mechanical findings are fixed, never suppressed by a verdict, whatever
    /// severity they are configured at.
    Unsuppressible,
}

impl Standing {
    /// The text the standing is stored as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Suppressed => "suppressed",
            Self::RuleChanged => "rule-changed",
            Self::EvidenceChanged => "evidence-changed",
            Self::Unsuppressible => "unsuppressible",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        [
            Self::Suppressed,
            Self::RuleChanged,
            Self::EvidenceChanged,
            Self::Unsuppressible,
        ]
        .into_iter()
        .find(|s| s.as_str() == text)
    }
}

/// The standing of a rejected finding with the reason it was rejected for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Judgment {
    pub standing: Standing,
    pub reason: Reason,
}

/// Where a finding is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    Resolved,
    Inactive,
    Suppressed,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
            Self::Inactive => "inactive",
            Self::Suppressed => "suppressed",
        })
    }
}

/// A finding as stored: its latest sighting, its history and its standing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingRecord {
    pub fingerprint: String,
    pub rule_id: String,
    pub severity: String,
    pub authored_severity: Option<String>,
    pub path: String,
    pub locator: Value,
    pub symbol: Option<String>,
    pub first_seen: String,
    pub last_seen: String,
    /// Set when a complete run no longer reported the finding.
    pub resolved_at: Option<String>,
    /// Set when the configuration no longer enables the finding's rule.
    pub inactive_at: Option<String>,
    /// How many times it came back after being resolved.
    pub reopened: u32,
    pub message: String,
    pub evidence: Value,
    pub facts: Value,
    pub options: Value,
    /// Commit and cleanliness of the working tree at the last sighting.
    pub commit: Option<String>,
    pub dirty: Option<bool>,
    pub lighthouse_version: Option<String>,
    pub catalog_version: Option<String>,
    pub rule_version: Option<String>,
    /// The latest verdict, if the finding was reviewed.
    pub review: Option<LatestReview>,
    /// What that verdict does to the finding now, if it was a rejection.
    pub standing: Option<Standing>,
    /// The rejection said the rule is too broad.
    pub narrowing: bool,
}

impl FindingRecord {
    /// Whether the finding asks for a verdict: its decision authored `warn` or
    /// `info`, whatever level it is reported at. A finding recorded when
    /// `review` was a severity of its own is one too.
    pub fn needs_verdict(&self) -> bool {
        matches!(self.authored_severity.as_deref(), Some("warn" | "info"))
            || self.severity == "review"
    }

    /// Where the finding is in its life; a suppression wins over the rest.
    pub fn state(&self) -> State {
        if self.standing == Some(Standing::Suppressed) {
            State::Suppressed
        } else if self.inactive_at.is_some() {
            State::Inactive
        } else if self.resolved_at.is_some() {
            State::Resolved
        } else {
            State::Open
        }
    }
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
    /// The commit the reviewed code was at, when known.
    pub commit: Option<String>,
    /// Refuse the verdict unless the finding's last sighting is this one.
    pub expect_seen: Option<String>,
    pub lighthouse_version: String,
}

/// What a review records about the rule beyond the finding: the versions of
/// the decision it judged and its scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stamp {
    /// Meaning version of the decision, which decides when the verdict expires.
    pub rule_version: Option<String>,
    /// How the decision was checked; recorded, never expires the verdict.
    pub check_revision: Option<String>,
    /// Hash of the whole decision definition.
    pub decision_hash: Option<String>,
    pub catalog_version: Option<String>,
    /// The decision's scope (`symbol`, `file`, ...).
    pub scope: Option<String>,
}

/// A recorded verdict with the finding as the verdict saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub event: ReviewEvent,
    pub finding: FindingRecord,
}

/// One entry of the decision log and of the review table: a verdict on a
/// finding with everything needed to learn from it later. `id` is the hash of
/// the rest, so the same entry read from two copies of the log is one entry.
/// This is the shape of the entries written before the resource model; the
/// log now writes them as `Verdict` records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewEvent {
    pub id: String,
    pub fingerprint: String,
    pub rule_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_version: Option<String>,
    /// How the decision was checked when the verdict was given; recorded for
    /// evaluation, never compared to expire the verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_revision: Option<String>,
    /// Read as `pattern_hash` in entries written before the rename.
    #[serde(
        default,
        rename = "pattern_hash",
        alias = "decision_hash",
        skip_serializing_if = "Option::is_none"
    )]
    pub decision_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighthouse_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern_fingerprint: Option<String>,
    pub verdict: Verdict,
    pub reason: Reason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_text: Option<String>,
    pub reviewer_kind: ReviewerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_digest: Option<String>,
    /// The finding frozen at review time: evidence, facts, options, severity,
    /// authored severity, when and where it was seen.
    pub snapshot: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub timestamp: String,
}

impl ReviewEvent {
    /// What the event teaches a model about its rule.
    pub fn label(&self) -> Label {
        Label::of(self.verdict, self.reason)
    }

    /// The finding's evidence when it was reviewed.
    pub fn evidence(&self) -> &Value {
        &self.snapshot["evidence"]
    }
}

/// A fix to record: what a fixer changed for a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFix {
    pub fingerprint: String,
    pub rule_id: String,
    pub fixer: String,
    /// `safe` or `suggested`.
    pub safety: String,
    pub description: String,
    /// Project-relative paths of the files the fix changed.
    pub files: Vec<String>,
    pub commit: Option<String>,
    pub lighthouse_version: String,
}

/// A recorded fix; `fixed by` the fixer named, when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixEvent {
    pub id: String,
    pub fingerprint: String,
    pub rule_id: String,
    pub fixer: String,
    pub safety: String,
    pub description: String,
    pub files: Vec<String>,
    pub commit: Option<String>,
    pub timestamp: String,
}
