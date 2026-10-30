//! Judgments: who is judging, which findings wait for one, and recording a
//! judgment on a finding. The store stays behind these functions.

use std::{env, path::Path};

use lighthouse_engine::{
    FixPlan,
    brief::{self, Target},
};
use lighthouse_model::{AgentKind, Attribution, Severity};
use lighthouse_plugin::FixDecision;
use lighthouse_report::{Aim, FixInfo};
use lighthouse_spec::{Catalog, Decision, FixKind, FixScope};
use lighthouse_store::{Filter, State, Store, Versions};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::{
    Fixing, Result, Selector, Session, git::user_name, project::catalog_at, selector::long_enough,
};

pub use lighthouse_store::{
    FindingRecord, JudgmentEvent, NewJudgment, Standing, StatusFilter, SuppressionEvent,
};

/// Says who is attributed with what is recorded when no flag does.
const ATTRIBUTED_VAR: &str = "LIGHTHOUSE_ATTRIBUTED_TO";

/// A judgment that was recorded, with what the reviewer should know about it:
/// the `ReviewResolve` kind.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Recorded {
    #[serde(rename = "recorded")]
    pub event: JudgmentEvent,
    #[serde(skip)]
    pub finding: FindingRecord,
    pub warnings: Vec<String>,
    /// What the judgment does to the finding now.
    pub standing: Option<Standing>,
    /// The catalog could not be read, so rule versions were not recorded.
    pub catalog_error: Option<String>,
}

/// Which remembered findings [`list`] gives.
pub struct Query {
    /// Only these decisions: ids or globs over them; empty for all.
    pub rules: Vec<String>,
    pub status: StatusFilter,
    /// Also the findings that are no task: errors, and open ones a judgment
    /// stands for.
    pub all_tiers: bool,
}

/// The remembered findings that wait for review.
pub struct Listed {
    pub findings: Vec<FindingRecord>,
    /// What opening the store skipped in the log, such as a last line an
    /// interrupted write cut short.
    pub notices: Vec<String>,
}

/// A finding as `review show` gives it: the record, how its fix runs, and the
/// brief for whoever fixes it by hand or with an agent of their own when its
/// fix is absent, `unsafe` or not running (the `Finding` kind).
#[derive(Debug, Serialize, JsonSchema)]
pub struct Shown {
    #[serde(flatten)]
    pub finding: FindingRecord,
    /// What the fix of this finding is told, written as the prompt of an
    /// `agent` run is: the decision, the finding, its scope and how the result
    /// is checked. Present when the descriptor of its decision says `brief`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    /// How the fix of the decision runs here, applied to this finding alone;
    /// absent when the decision has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<FixInfo>,
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
pub fn list(root: &Path, query: &Query) -> Result<Option<Listed>> {
    let Some(store) = Store::open_existing(root)? else {
        return Ok(None);
    };
    let selector = Selector::new(&query.rules)?;
    let found = store.list(&Filter {
        decision: None,
        status: query.status,
    })?;
    Ok(Some(Listed {
        findings: found
            .into_iter()
            .filter(|f| selector.matches(&f.decision))
            .filter(|f| query.all_tiers || waits(f, query.status))
            .collect(),
        notices: store.notices().to_vec(),
    }))
}

/// The finding whose fingerprint starts with `fingerprint`, as remembered.
pub fn show(root: &Path, fingerprint: &str) -> Result<FindingRecord> {
    long_enough(fingerprint)?;
    Ok(existing_store(root)?.finding(fingerprint)?)
}

/// The finding whose fingerprint starts with `fingerprint`, with how its fix
/// runs and its brief when `session` (the project, if it can be read) knows
/// its decision.
pub fn show_with_brief(root: &Path, session: Option<&Session>, fingerprint: &str) -> Result<Shown> {
    let finding = show(root, fingerprint)?;
    let Some((session, decision)) =
        session.and_then(|s| Some((s, s.catalog.decision(&finding.decision)?)))
    else {
        return Ok(Shown {
            finding,
            brief: None,
            fix: None,
        });
    };
    let fingerprints = [finding.fingerprint.as_str()];
    let described = Fixing::of(session)?.describe(decision, Aim::Findings(&fingerprints));
    let brief = match &described {
        Some(descriptor) if descriptor.brief => Some(brief_of(session, decision, &finding)?),
        _ => None,
    };
    Ok(Shown {
        finding,
        brief,
        fix: described.and_then(|descriptor| descriptor.fix),
    })
}

/// Every judgment recorded on a finding, oldest first.
pub fn history(root: &Path, fingerprint: &str) -> Result<Vec<JudgmentEvent>> {
    long_enough(fingerprint)?;
    Ok(existing_store(root)?.history(fingerprint)?)
}

/// Deletes the resolved and inactive findings nobody judged, only those that
/// have been for `older_than` days when given; returns how many.
pub fn prune(root: &Path, older_than: Option<u32>) -> Result<usize> {
    Ok(existing_store(root)?.prune(older_than)?)
}

/// Records the judgment, stamping it with the versions of the finding's
/// decision when the catalog can be read; a broken catalog is reported, not
/// fatal.
pub fn resolve(root: &Path, review: &NewJudgment) -> Result<Recorded> {
    long_enough(&review.fingerprint)?;
    resolve_all(root, std::slice::from_ref(review))?.remove(0)
}

/// Records the judgments one after the other, opening the store and reading
/// the catalog once. Each judgment succeeds or fails by itself.
pub fn resolve_all(root: &Path, reviews: &[NewJudgment]) -> Result<Vec<Result<Recorded>>> {
    let mut store = existing_store(root)?;
    let (catalog, catalog_error) = match catalog_at(root) {
        Ok(catalog) => (Some(catalog), None),
        Err(e) => (None, Some(e.to_string())),
    };
    let record = |store: &mut Store, review: &NewJudgment| -> Result<Recorded> {
        let resolved = store.resolve(review, |finding| stamp(catalog.as_ref(), finding))?;
        let standing = store.finding(&resolved.event.spec.fingerprint)?.standing;
        Ok(Recorded {
            warnings: warnings(&resolved.finding),
            event: resolved.event,
            finding: resolved.finding,
            standing,
            catalog_error: catalog_error.clone(),
        })
    };
    Ok(reviews
        .iter()
        .map(|review| record(&mut store, review))
        .collect())
}

/// The brief for fixing `finding` by `decision`.
fn brief_of(session: &Session, decision: &Decision, finding: &FindingRecord) -> Result<String> {
    let told = FixDecision {
        id: decision.id().to_owned(),
        requirement: decision.requirement.clone(),
        context: decision.context.clone(),
    };
    let target = Target {
        decision: &told,
        instruction: match decision.fix.as_ref().map(|fix| &fix.kind) {
            Some(FixKind::Model { prompt, .. }) => prompt.as_deref(),
            _ => None,
        },
        examples: brief::examples_for(&decision.examples),
        scope: FixScope::of_fix(decision.fix.as_ref(), session.config.fix().scope),
        message: &finding.message,
        evidence: &finding.evidence,
        place: format!("{}:{}:{}", finding.path, finding.line(), finding.col()),
    };
    let plan = FixPlan::from_catalog(&session.catalog, &session.base);
    let active = crate::active_decisions(session)?;
    let tools: Vec<&str> = brief::safe_fixes(&plan, None)
        .into_iter()
        .filter(|id| active.contains(*id))
        .collect();
    Ok(brief::brief(&[target], &tools, None))
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

fn stamp(catalog: Option<&Catalog>, finding: &FindingRecord) -> Versions {
    let Some(catalog) = catalog else {
        return Versions::default();
    };
    let decision = catalog.decision(&finding.decision);
    Versions {
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
    match finding.state {
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
