use std::fmt;

use lighthouse_model::{
    Attribution, Diagnostic, Judgment, Label, Severity, SuppressionKind, SuppressionStatus,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::digest;

/// One finding as a run saw it. The locator and evidence are JSON so that
/// artifacts other than source code fit the same record.
#[derive(Debug, Clone, PartialEq)]
pub struct Observed {
    pub fingerprint: String,
    pub rule_id: String,
    /// The uid of the finding's decision.
    pub decision_uid: Option<String>,
    pub severity: Severity,
    /// The severity its decision authored: what decides whether a judgment may
    /// hide the finding. For a rule without a decision, the severity it
    /// reported.
    pub authored_severity: Severity,
    /// The artifact, project-relative with `/` separators.
    pub path: String,
    /// Where in the artifact: `{"span": {"start": {"line", "col"}, "end": ...}}`
    /// for text.
    pub locator: Value,
    pub symbol: Option<String>,
    pub message: String,
    pub evidence: Value,
    /// What the analysis knew about the subject: language, symbol shape,
    /// measures. Frozen into a judgment's feature snapshot.
    pub facts: Value,
    /// The options the rule ran with, defaults included.
    pub options: Value,
    /// Hash of what the rule's decision means; judgments expire when it moves.
    pub meaning_version: Option<String>,
    /// Hash of how the decision is checked. Recorded, never compared to
    /// expire a judgment.
    pub check_revision: Option<String>,
    /// Hash of the whole decision definition, wording and examples included.
    pub decision_hash: Option<String>,
}

impl Observed {
    /// The record of a diagnostic, with the facts the analysis gathered for
    /// it. The decision's uid, options and versions start unknown; set them
    /// directly.
    pub fn from_diagnostic(diagnostic: &Diagnostic, facts: Value) -> Self {
        Self {
            fingerprint: diagnostic.fingerprint.as_str().to_owned(),
            rule_id: diagnostic.rule_id.clone(),
            decision_uid: None,
            severity: diagnostic.severity,
            authored_severity: diagnostic.severity,
            path: diagnostic.file.to_string_lossy().replace('\\', "/"),
            locator: json!({ "span": diagnostic.span }),
            symbol: diagnostic.symbol.clone(),
            message: diagnostic.message.clone(),
            evidence: diagnostic.evidence.clone(),
            facts,
            options: json!({}),
            meaning_version: None,
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
    /// Still reported: not resolved, inactive or kept out by a judgment.
    Open,
    /// Kept out of reports by their latest judgment.
    Suppressed,
    /// Judged `notApplicable`: the decision should be narrowed.
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

/// What the latest judgment does to a finding now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Standing {
    /// The finding stays out of reports.
    Suppressed,
    /// The finding is judged and stays in the report (`fail` with no
    /// suppression).
    Judged,
    /// The decision's meaning changed since the judgment: ask again.
    RuleChanged,
    /// The finding's evidence changed since the judgment: ask again.
    EvidenceChanged,
    /// Errors are definitive: no judgment hides them, whatever level they
    /// are configured at.
    Unsuppressible,
}

impl Standing {
    const ALL: [Self; 5] = [
        Self::Suppressed,
        Self::Judged,
        Self::RuleChanged,
        Self::EvidenceChanged,
        Self::Unsuppressible,
    ];

    /// The text the standing is stored as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Suppressed => "suppressed",
            Self::Judged => "judged",
            Self::RuleChanged => "rule-changed",
            Self::EvidenceChanged => "evidence-changed",
            Self::Unsuppressible => "unsuppressible",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == text)
    }

    /// Whether the judgment still stands for the finding: it neither expired
    /// nor was refused.
    pub fn stands(self) -> bool {
        matches!(self, Self::Suppressed | Self::Judged)
    }
}

/// The standing of a judged finding with the judgment behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ruling {
    pub standing: Standing,
    pub judgment: Judgment,
    /// The justification of the suppression that goes with it, if any.
    pub justification: Option<String>,
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
    /// The uid of the finding's decision, when the catalog gave it one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_uid: Option<String>,
    pub severity: Severity,
    pub authored_severity: Severity,
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
    pub meaning_version: Option<String>,
    /// The latest judgment, if the finding was judged.
    pub judgment: Option<Judgment>,
    /// What that judgment does to the finding now.
    pub standing: Option<Standing>,
    /// The justification of the suppression that goes with the judgment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub justification: Option<String>,
    /// The judgment said the decision does not apply here.
    pub narrowing: bool,
}

impl FindingRecord {
    /// Whether the finding asks for review: its decision authored `warn` or
    /// `info`, and no judgment stands for it.
    pub fn needs_review(&self) -> bool {
        self.authored_severity
            .needs_review(self.standing.is_some_and(Standing::stands))
    }

    /// Where the finding is in its life; a judgment that hides it wins over
    /// the rest.
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

/// A judgment to record. `fingerprint` may be an unambiguous prefix.
#[derive(Debug, Clone, PartialEq)]
pub struct NewJudgment {
    pub fingerprint: String,
    pub judgment: Judgment,
    /// Why, written for the next reader.
    pub reason: Option<String>,
    /// The justification of an external suppression recorded with a `fail`.
    pub suppress: Option<String>,
    pub attribution: Attribution,
    /// The commit the judged code was at, when known.
    pub commit: Option<String>,
    /// Refuse the judgment unless the finding's last sighting is this one.
    pub expect_seen: Option<String>,
    pub lighthouse_version: String,
}

/// What a judgment records about the decision beyond the finding: the versions
/// of the decision it judged and its scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stamp {
    /// The uid of the finding's decision, when it differs from what the
    /// finding was recorded with.
    pub decision_uid: Option<String>,
    /// Meaning version of the decision, which decides when the judgment expires.
    pub meaning_version: Option<String>,
    /// How the decision was checked; recorded, never expires the judgment.
    pub check_revision: Option<String>,
    /// Hash of the whole decision definition.
    pub decision_hash: Option<String>,
    pub catalog_version: Option<String>,
    /// The decision's scope (`symbol`, `file`, ...).
    pub scope: Option<String>,
}

/// A recorded judgment with the finding as the judgment saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub event: JudgmentEvent,
    pub finding: FindingRecord,
}

/// A suppression that went with a judgment.
#[derive(Debug, Clone, PartialEq)]
pub struct SuppressionEvent {
    pub id: String,
    pub kind: SuppressionKind,
    pub status: SuppressionStatus,
    pub justification: String,
    pub was_attributed_to: Attribution,
    pub generated_at_time: String,
}

/// One entry of the decision log and of the judgments table: a judgment on a
/// finding with everything needed to learn from it later. `id` is the hash of
/// the record's spec, so the same entry read from two copies of the log is one
/// entry.
#[derive(Debug, Clone, PartialEq)]
pub struct JudgmentEvent {
    pub id: String,
    pub fingerprint: String,
    /// The name the decision had when the judgment was given; `decision_uid`
    /// is what identifies it.
    pub decision_name: String,
    pub decision_uid: Option<String>,
    pub meaning_version: Option<String>,
    /// How the decision was checked; recorded for evaluation, never compared
    /// to expire the judgment.
    pub check_revision: Option<String>,
    pub decision_hash: Option<String>,
    pub catalog_version: Option<String>,
    pub lighthouse_version: Option<String>,
    pub judgment: Judgment,
    pub reason: Option<String>,
    pub was_attributed_to: Attribution,
    pub generated_at_time: String,
    pub language: Option<String>,
    pub scope: Option<String>,
    pub evidence_digest: Option<String>,
    /// The finding frozen at judgment time: evidence, facts, options,
    /// severities, when and where it was seen.
    pub snapshot: Value,
    pub commit: Option<String>,
    pub suppressions: Vec<SuppressionEvent>,
}

impl JudgmentEvent {
    /// What the judgment teaches a model about its decision.
    pub fn label(&self) -> Label {
        Label::of(
            self.judgment,
            self.suppressions
                .iter()
                .any(|s| s.status == SuppressionStatus::Accepted),
        )
    }

    /// The finding's evidence when it was judged.
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
