//! Why a remembered finding is as it is: the layers its severity went
//! through, the judgment that hides it or not, and what the project decided
//! about the same decision before. `review show --detail full` gives it.

use std::path::Path;

use lighthouse_model::{Attribution, Fingerprint, Judgment, RunScope};
use lighthouse_spec::{Stage, Step};
use lighthouse_store::{Filter, FindingRecord, Ruling, Standing, StatusFilter, Store};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::{Result, Session, findings};

/// Precedents listed with their reasons.
const RECENT: usize = 3;

/// The latest judgment on a finding, and what it does to the finding now.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Hiding {
    pub judgment: Judgment,
    pub by: Attribution,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The justification of the `external` suppression that goes with a `fail`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suppression: Option<String>,
    pub standing: Standing,
    /// Why the judgment hides the finding or does not.
    pub note: String,
}

/// The judgments on the other findings of the same decision.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Precedents {
    pub pass: usize,
    pub fail: usize,
    pub not_applicable: usize,
    /// The most recent that gave a reason, newest first.
    pub recent: Vec<Precedent>,
}

/// A judgment on another finding of the decision.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Precedent {
    /// The first characters of the finding's fingerprint.
    pub fingerprint: String,
    pub path: String,
    pub judgment: Judgment,
    pub reason: String,
    pub at: String,
}

/// How the severity of `finding` was reached: the decision's own, then each
/// layer of the project that set the level of the rule where the finding is
/// (a decision judged over the project is resolved at no path, as the engine
/// does), the last being the effective one. Without a session only the decision's own
/// is known.
pub fn precedence(session: Option<&Session>, finding: &FindingRecord) -> Result<Vec<Step>> {
    let authored = Step {
        stage: Stage::Authored,
        source: finding.decision.clone(),
        matched: None,
        level: Some(finding.authored_severity).into(),
    };
    let Some(session) = session else {
        return Ok(vec![authored]);
    };
    let judged_over_project = session
        .catalog
        .decision(&finding.decision)
        .is_some_and(|d| d.scope.subject.run_scope() == RunScope::Project);
    let (path, language) = if judged_over_project {
        (Path::new(""), "")
    } else {
        let language = finding.facts.get("language").and_then(Value::as_str);
        (Path::new(&finding.path), language.unwrap_or_default())
    };
    let rules = session
        .config
        .resolve(path, language, session.catalog.projects())?;
    let steps = rules
        .get(&finding.decision)
        .map(|rule| rule.trace.0.clone())
        .unwrap_or_default();
    Ok(std::iter::once(authored).chain(steps).collect())
}

/// What the latest judgment on `finding` does to it; nothing when it was never
/// judged.
pub fn hiding(store: &Store, finding: &FindingRecord) -> Result<Vec<Hiding>> {
    let (Some(standing), Some(judgment)) = (finding.standing, finding.judgment) else {
        return Ok(Vec::new());
    };
    let Some(event) = store.history(&finding.fingerprint)?.pop() else {
        return Ok(Vec::new());
    };
    let suppression = event
        .suppressions
        .first()
        .map(|s| s.spec.justification.clone());
    Ok(vec![Hiding {
        judgment,
        by: event.spec.was_attributed_to,
        at: event.spec.generated_at_time,
        reason: event.spec.reason,
        note: note(standing, judgment, finding.justification.clone()),
        suppression,
        standing,
    }])
}

/// The judgments on the other findings of `finding`'s decision.
pub fn precedents(store: &Store, finding: &FindingRecord) -> Result<Precedents> {
    let others = store.list(&Filter {
        decision: Some(finding.decision.clone()),
        status: StatusFilter::All,
    })?;
    let mut out = Precedents::default();
    let mut recent = Vec::new();
    for other in others
        .into_iter()
        .filter(|other| other.fingerprint != finding.fingerprint)
    {
        let Some(judgment) = other.judgment else {
            continue;
        };
        match judgment {
            Judgment::Pass => out.pass += 1,
            Judgment::Fail => out.fail += 1,
            Judgment::NotApplicable => out.not_applicable += 1,
        }
        if let Some(event) = store.history(&other.fingerprint)?.pop()
            && let Some(reason) = event.spec.reason
        {
            recent.push(Precedent {
                fingerprint: other.fingerprint[..Fingerprint::MIN_PREFIX].to_owned(),
                path: other.path.clone(),
                judgment,
                reason,
                at: event.spec.generated_at_time,
            });
        }
    }
    recent.sort_by(|a, b| b.at.cmp(&a.at));
    recent.truncate(RECENT);
    out.recent = recent;
    Ok(out)
}

/// Why a judgment with this `standing` hides the finding or does not.
fn note(standing: Standing, judgment: Judgment, justification: Option<String>) -> String {
    match standing {
        Standing::Suppressed => {
            "hidden: the judgment stands while its decision and evidence stay as they are"
                .to_owned()
        }
        Standing::Judged => "reported: a `fail` hides nothing without a suppression".to_owned(),
        other => findings::note(&Ruling {
            standing: other,
            judgment,
            justification,
        })
        .unwrap_or_default(),
    }
}
