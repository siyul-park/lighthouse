//! Review verdicts: who is reviewing, and recording a verdict on a finding.

use std::{env, path::Path};

use lighthouse_model::ReviewerKind;
use lighthouse_spec::{Catalog, Pattern};
use lighthouse_store::{FindingRecord, NewReview, ReviewEvent, Stamp, Standing, State, Store};
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

/// The store, which must already exist: nothing is remembered before a check.
pub fn existing_store(root: &Path) -> Result<Store> {
    Store::open_existing(root)?
        .ok_or_else(|| "no findings recorded yet (run `lighthouse check`)".into())
}

/// Records the verdict, stamping it with the versions of the finding's rule
/// when the catalog can be read; a broken catalog is reported, not fatal.
pub fn record_verdict(root: &Path, store: &mut Store, review: &NewReview) -> Result<Recorded> {
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

fn stamp(catalog: Option<&Catalog>, finding: &FindingRecord) -> Stamp {
    let Some(catalog) = catalog else {
        return Stamp::default();
    };
    let pattern = catalog.pattern(&finding.rule_id);
    Stamp {
        rule_version: pattern.map(Pattern::semantic_version),
        pattern_hash: pattern.map(Pattern::version),
        catalog_version: Some(catalog.version()),
        scope: pattern.map(|p| p.scope.to_string()),
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
