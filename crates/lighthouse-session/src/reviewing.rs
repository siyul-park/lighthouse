//! Judgments: who is judging, which findings wait for one, and recording a
//! judgment on a finding. The store stays behind these functions.

use std::{env, path::Path};

use lighthouse_model::{AgentKind, Attribution, Severity};
use lighthouse_spec::{Catalog, Decision};
use lighthouse_store::{
    Filter, FindingRecord, JudgmentEvent, NewJudgment, Stamp, Standing, State, StatusFilter, Store,
};
use serde_json::Value;

use crate::{Result, project::catalog_at};

/// Environment variables that say who is reviewing when nothing else does.
/// Tools that run reviews for an agent (hooks, the MCP server) set both.
const KIND_VAR: &str = "LIGHTHOUSE_REVIEWER_KIND";
const ID_VAR: &str = "LIGHTHOUSE_REVIEWER";

/// Who is judging.
pub struct Reviewer {
    pub kind: AgentKind,
    pub id: Option<String>,
}

impl Reviewer {
    /// The given kind and id, else `$LIGHTHOUSE_REVIEWER_KIND` (default human)
    /// and `$LIGHTHOUSE_REVIEWER`, else `$USER`.
    pub fn from_env(kind: Option<AgentKind>, id: Option<String>) -> Result<Self> {
        let kind = match (kind, env::var(KIND_VAR)) {
            (Some(kind), _) => kind,
            (None, Ok(text)) => text.parse().map_err(|e| format!("{KIND_VAR}: {e}"))?,
            (None, Err(_)) => AgentKind::Person,
        };
        let id = id
            .or_else(|| env::var(ID_VAR).ok())
            .or_else(|| env::var("USER").ok())
            .filter(|id| !id.is_empty());
        Ok(Self { kind, id })
    }

    /// The reviewer as a record attributes a judgment.
    pub fn attribution(&self) -> Attribution {
        Attribution {
            kind: self.kind,
            id: self.id.clone(),
        }
    }
}

/// A judgment that was recorded, with what the reviewer should know about it.
pub struct Recorded {
    pub event: JudgmentEvent,
    pub finding: FindingRecord,
    pub warnings: Vec<String>,
    /// What the judgment does to the finding now.
    pub standing: Option<Standing>,
    /// The catalog could not be read, so rule versions were not recorded.
    pub catalog_error: Option<String>,
}

/// Which remembered findings [`review_tasks`] lists.
pub struct TaskQuery {
    /// Only this fully qualified rule id.
    pub rule: Option<String>,
    pub status: StatusFilter,
    /// Also the findings that are no task: errors, and open ones a judgment
    /// stands for.
    pub all_tiers: bool,
}

/// The remembered findings that wait for review.
pub struct Tasks {
    pub findings: Vec<FindingRecord>,
}

/// The findings the store remembers that match `query`; `None` when no check
/// has recorded anything yet.
pub fn review_tasks(root: &Path, query: &TaskQuery) -> Result<Option<Tasks>> {
    let Some(store) = Store::open_existing(root)? else {
        return Ok(None);
    };
    let found = store.list(&Filter {
        rule: query.rule.clone(),
        status: query.status,
    })?;
    Ok(Some(Tasks {
        findings: found
            .into_iter()
            .filter(|f| query.all_tiers || waits(f, query.status))
            .collect(),
    }))
}

/// Whether a finding is a task of a listing: among the open ones, those that
/// ask for review; in any other status, those of decisions that did not
/// author an error, judged or not.
fn waits(finding: &FindingRecord, status: StatusFilter) -> bool {
    if status == StatusFilter::Open {
        finding.needs_review()
    } else {
        finding.authored_severity != Severity::Error
    }
}

/// The finding whose fingerprint starts with `fingerprint`, as remembered.
pub fn review_finding(root: &Path, fingerprint: &str) -> Result<FindingRecord> {
    Ok(existing_store(root)?.finding(fingerprint)?)
}

/// Every judgment recorded on a finding, oldest first.
pub fn review_history(root: &Path, fingerprint: &str) -> Result<Vec<JudgmentEvent>> {
    Ok(existing_store(root)?.history(fingerprint)?)
}

/// Deletes the resolved and inactive findings nobody judged, only those that
/// have been for `older_than` days when given; returns how many.
pub fn review_prune(root: &Path, older_than: Option<u32>) -> Result<usize> {
    Ok(existing_store(root)?.prune(older_than)?)
}

/// Records the judgment, stamping it with the versions of the finding's
/// decision when the catalog can be read; a broken catalog is reported, not
/// fatal.
pub fn record_judgment(root: &Path, review: &NewJudgment) -> Result<Recorded> {
    let mut store = existing_store(root)?;
    let store = &mut store;
    let (catalog, catalog_error) = match catalog_at(root) {
        Ok(catalog) => (Some(catalog), None),
        Err(e) => (None, Some(e.to_string())),
    };
    let resolved = store.resolve(review, |finding| stamp(catalog.as_ref(), finding))?;
    let standing = store
        .standings()?
        .get(&resolved.event.fingerprint)
        .map(|j| j.standing);
    Ok(Recorded {
        warnings: warnings(&resolved.finding),
        event: resolved.event,
        finding: resolved.finding,
        standing,
        catalog_error,
    })
}

/// The store, which must already exist: nothing is remembered before a check.
fn existing_store(root: &Path) -> Result<Store> {
    Store::open_existing(root)?
        .ok_or_else(|| "no findings recorded yet (run `lighthouse check`)".into())
}

fn stamp(catalog: Option<&Catalog>, finding: &FindingRecord) -> Stamp {
    let Some(catalog) = catalog else {
        return Stamp::default();
    };
    let decision = catalog.decision(&finding.rule_id);
    Stamp {
        decision_uid: decision.and_then(|d| d.uid()).map(str::to_owned),
        meaning_version: decision.map(|d| d.meaning_version()),
        check_revision: decision.map(|d| d.check_revision()),
        decision_hash: decision.map(Decision::version),
        catalog_version: Some(catalog.version()),
        scope: decision.map(|d| d.scope.subject.to_string()),
    }
}

/// What the reviewer should know about the finding they just judged.
fn warnings(finding: &FindingRecord) -> Vec<String> {
    let mut warnings = Vec::new();
    match finding.state() {
        State::Resolved => warnings.push(
            "the finding was already resolved: no complete run reports it any more, so the judgment only matters if it comes back".to_owned(),
        ),
        State::Inactive => warnings.push(
            "the finding's rule is no longer enabled by the configuration".to_owned(),
        ),
        State::Open | State::Suppressed => {}
    }
    if finding.facts.get("ordinal") == Some(&Value::Bool(true)) {
        warnings.push(
            "this finding's identity rests on its position among identical findings, so the judgment may stop matching when one of them is added or removed".to_owned(),
        );
    }
    warnings
}
