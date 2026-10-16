//! Judgments: who is judging, which findings wait for one, and recording a
//! judgment on a finding. The store stays behind these functions.

use std::{env, path::Path};

use lighthouse_model::{AgentKind, Attribution, Severity};
use lighthouse_spec::{Catalog, Decision};
use lighthouse_store::{
    Filter, FindingRecord, JudgmentEvent, NewJudgment, Stamp, Standing, State, StatusFilter, Store,
};
use serde_json::Value;

use crate::{Result, Selector, git::user_name, project::catalog_at};

/// Says who is attributed with what is recorded when no flag does.
const ATTRIBUTED_VAR: &str = "LIGHTHOUSE_ATTRIBUTED_TO";

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
    /// Only these decisions: ids or globs over them; empty for all.
    pub rules: Vec<String>,
    pub status: StatusFilter,
    /// Also the findings that are no task: errors, and open ones a judgment
    /// stands for.
    pub all_tiers: bool,
}

/// The remembered findings that wait for review.
pub struct Tasks {
    pub findings: Vec<FindingRecord>,
    /// What opening the store skipped in the log, such as a last line an
    /// interrupted write cut short.
    pub notices: Vec<String>,
}

/// Who a record is attributed to (PROV `wasAttributedTo`): `spec` is
/// `person`, `person:<id>`, `agent` or `agent:<id>`; without it
/// `$LIGHTHOUSE_ATTRIBUTED_TO` says; without that `default` does. A missing id
/// is the default's when it is of the same kind.
pub fn attribute_to(spec: Option<&str>, default: &Attribution) -> Result<Attribution> {
    let from_env = env::var(ATTRIBUTED_VAR)
        .ok()
        .filter(|text| !text.is_empty());
    let Some(text) = spec.map(str::to_owned).or(from_env) else {
        return Ok(default.clone());
    };
    let (kind, id) = match text.split_once(':') {
        Some((kind, id)) => (kind, Some(id)),
        None => (text.as_str(), None),
    };
    let kind = match kind {
        "person" => AgentKind::Person,
        "agent" => AgentKind::SoftwareAgent,
        other => {
            return Err(
                format!("`{other}` is not person or agent (expected person|agent[:<id>])").into(),
            );
        }
    };
    let id = match id.filter(|id| !id.is_empty()) {
        Some(id) => Some(id.to_owned()),
        None if default.kind == kind => default.id.clone(),
        None => None,
    };
    Ok(Attribution::new(kind, id))
}

/// The person at the keyboard: the name configured for the repository, else
/// `$USER`.
pub fn person(root: &Path) -> Attribution {
    let id = user_name(root).or_else(|| env::var("USER").ok());
    Attribution::new(AgentKind::Person, id.filter(|id| !id.is_empty()))
}

/// The findings the store remembers that match `query`; `None` when no check
/// has recorded anything yet.
pub fn review_tasks(root: &Path, query: &TaskQuery) -> Result<Option<Tasks>> {
    let Some(store) = Store::open_existing(root)? else {
        return Ok(None);
    };
    let selector = Selector::new(&query.rules)?;
    let found = store.list(&Filter {
        rule: None,
        status: query.status,
    })?;
    Ok(Some(Tasks {
        findings: found
            .into_iter()
            .filter(|f| selector.matches(&f.rule_id))
            .filter(|f| query.all_tiers || waits(f, query.status))
            .collect(),
        notices: store.notices().to_vec(),
    }))
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
    let standing = store.finding(&resolved.event.fingerprint)?.standing;
    Ok(Recorded {
        warnings: warnings(&resolved.finding),
        event: resolved.event,
        finding: resolved.finding,
        standing,
        catalog_error,
    })
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
        catalog_version: Some(catalog.version().to_owned()),
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
