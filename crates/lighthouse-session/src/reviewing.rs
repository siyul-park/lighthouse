//! Review verdicts: who is reviewing, which findings wait for one, and
//! recording a verdict on a finding. The store stays behind these functions.

use std::{env, path::Path};

use lighthouse_model::ReviewerKind;
use lighthouse_spec::{Catalog, Decision};
use lighthouse_store::{
    Filter, FindingRecord, NewReview, ReviewEvent, Stamp, Standing, State, StatusFilter, Store,
};
use serde_json::Value;

use crate::{Result, project::catalog_at};

/// Environment variables that say who is reviewing when nothing else does.
/// Tools that run reviews for an agent (hooks, the MCP server) set both.
const KIND_VAR: &str = "LIGHTHOUSE_REVIEWER_KIND";
const ID_VAR: &str = "LIGHTHOUSE_REVIEWER";

/// Who is reviewing.
pub struct Reviewer {
    pub kind: ReviewerKind,
    pub id: Option<String>,
}

impl Reviewer {
    /// The given kind and id, else `$LIGHTHOUSE_REVIEWER_KIND` (default human)
    /// and `$LIGHTHOUSE_REVIEWER`, else `$USER`.
    pub fn from_env(kind: Option<ReviewerKind>, id: Option<String>) -> Result<Self> {
        let kind = match (kind, env::var(KIND_VAR)) {
            (Some(kind), _) => kind,
            (None, Ok(text)) => text.parse().map_err(|e| format!("{KIND_VAR}: {e}"))?,
            (None, Err(_)) => ReviewerKind::Human,
        };
        let id = id
            .or_else(|| env::var(ID_VAR).ok())
            .or_else(|| env::var("USER").ok())
            .filter(|id| !id.is_empty());
        Ok(Self { kind, id })
    }
}

/// A verdict that was recorded, with what the reviewer should know about it.
pub struct Recorded {
    pub event: ReviewEvent,
    pub finding: FindingRecord,
    pub warnings: Vec<String>,
    /// What the verdict does to the finding now.
    pub standing: Option<Standing>,
    /// The catalog could not be read, so rule versions were not recorded.
    pub catalog_error: Option<String>,
}

/// Which remembered findings [`review_tasks`] lists.
pub struct TaskQuery {
    /// Only this fully qualified rule id.
    pub rule: Option<String>,
    pub status: StatusFilter,
    /// Also the findings that do not ask for a verdict (mechanical decisions).
    pub all_tiers: bool,
}

/// The remembered findings that wait for a verdict.
pub struct Tasks {
    pub findings: Vec<FindingRecord>,
    /// What opening the store had to say, such as a log line it skipped.
    pub notices: Vec<String>,
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
            .filter(|f| query.all_tiers || f.needs_verdict())
            .collect(),
        notices: store.notices().to_vec(),
    }))
}

/// The finding whose fingerprint starts with `fingerprint`, as remembered.
pub fn review_finding(root: &Path, fingerprint: &str) -> Result<FindingRecord> {
    Ok(existing_store(root)?.finding(fingerprint)?)
}

/// Every verdict recorded on a finding, oldest first.
pub fn review_history(root: &Path, fingerprint: &str) -> Result<Vec<ReviewEvent>> {
    Ok(existing_store(root)?.history(fingerprint)?)
}

/// Deletes the resolved and inactive findings nobody reviewed, only those that
/// have been for `older_than` days when given; returns how many.
pub fn review_prune(root: &Path, older_than: Option<u32>) -> Result<usize> {
    Ok(existing_store(root)?.prune(older_than)?)
}

/// Records the verdict, stamping it with the versions of the finding's rule
/// when the catalog can be read; a broken catalog is reported, not fatal.
pub fn record_verdict(root: &Path, review: &NewReview) -> Result<Recorded> {
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
        rule_version: decision.map(|d| d.meaning_version()),
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
            "the finding was already resolved: no complete run reports it any more, so the verdict only matters if it comes back".to_owned(),
        ),
        State::Inactive => warnings.push(
            "the finding's rule is no longer enabled by the configuration".to_owned(),
        ),
        State::Open | State::Suppressed => {}
    }
    if finding.facts.get("ordinal") == Some(&Value::Bool(true)) {
        warnings.push(
            "this finding's identity rests on its position among identical findings, so the verdict may stop matching when one of them is added or removed".to_owned(),
        );
    }
    warnings
}
