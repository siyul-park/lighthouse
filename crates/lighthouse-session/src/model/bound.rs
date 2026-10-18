//! The models a project binds to its decisions.

use std::{collections::BTreeMap, path::Path};

use lighthouse_model::ModelRef;
use lighthouse_spec::Catalog;
use schemars::JsonSchema;
use serde::Serialize;

use crate::{Result, Selector, Session};

/// Where a binding comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Binding {
    /// `models.classification`: the model for decisions nothing else binds.
    Classification,
    /// `models.agent`: the agent that fixes by changing files.
    Edit,
    /// A rule of the configuration names it for classification.
    #[serde(rename = "rule-classification")]
    RuleClassification,
    /// A rule of the configuration names it for the agent that edits.
    #[serde(rename = "rule-edit")]
    RuleEdit,
    /// A trained model attached by an approved revision.
    Attached,
}

/// One model and what it is bound to.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BoundModel {
    /// `<provider>/<model>`.
    pub model: String,
    /// The decision it judges; none for the classification and edit
    /// models.
    pub decision: Option<String>,
    pub binding: Binding,
    /// Whether asking it reaches a service outside the machine.
    pub remote: bool,
}

/// The models of the project bound to decisions matching `decisions` (ids and
/// globs; all when empty), in a stable order.
pub fn list(session: &Session, decisions: &[String]) -> Result<Vec<BoundModel>> {
    let catalog = &session.catalog;
    let bound = session.config.models(catalog.projects())?;
    let known: Vec<&str> = catalog.decisions().map(|d| d.id()).collect();
    let selector = Selector::new(decisions)?;
    selector.expand(known.iter().copied())?;
    let remote = |model: &str| {
        ModelRef::parse(model)
            .and_then(|model| bound.provider(model.provider))
            .is_some_and(|provider| provider.remote())
    };
    let mut found = Vec::new();
    let mut bind = |model: &String, decision: Option<&str>, binding| {
        found.push(BoundModel {
            model: model.clone(),
            decision: decision.map(str::to_owned),
            binding,
            remote: remote(model),
        });
    };
    if selector.is_empty() {
        for model in &bound.classification {
            bind(model, None, Binding::Classification);
        }
        for model in &bound.agent {
            bind(model, None, Binding::Edit);
        }
    }
    for (decision, models) in bound.routes() {
        if selector.matches(decision) {
            for model in models {
                bind(model, Some(decision), Binding::RuleClassification);
            }
        }
    }
    for (decision, models) in bound.agent_routes() {
        if selector.matches(decision) {
            for model in models {
                bind(model, Some(decision), Binding::RuleEdit);
            }
        }
    }
    for (decision, model) in &attached(&session.root, catalog) {
        if selector.matches(decision) {
            bind(model, Some(decision.as_str()), Binding::Attached);
        }
    }
    Ok(found)
}

/// Whether the project binds a model that reaches a service outside the
/// machine.
pub fn remote_bound(session: &Session) -> bool {
    list(session, &[]).is_ok_and(|models| models.iter().any(|m| m.remote))
}

/// The model each decision is bound to by an approved revision, by decision
/// id.
pub(crate) fn attached(root: &Path, catalog: &Catalog) -> BTreeMap<String, String> {
    let Ok(records) = lighthouse_store::Records::read(root) else {
        return BTreeMap::new();
    };
    let named = |key: &str| {
        catalog
            .decisions()
            .find(|d| d.is_key(key))
            .map(|d| d.id().to_owned())
    };
    records
        .attached()
        .into_iter()
        .filter_map(|(decision, model)| Some((named(&decision)?, model)))
        .collect()
}
