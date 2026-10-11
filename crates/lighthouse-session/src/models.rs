//! The models a project binds to its decisions.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{Result, Selector, Session, attached_models};

/// Where a binding comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Binding {
    /// `models.classification`: the model for decisions nothing else binds.
    Classification,
    /// A rule of the configuration names it.
    Rule,
    /// A trained model attached by an approved revision.
    Attached,
}

/// One model and what it is bound to.
#[derive(Debug, Clone, Serialize)]
pub struct BoundModel {
    /// `<provider>/<model>`.
    pub model: String,
    /// The decision it judges; none for the classification model.
    pub decision: Option<String>,
    pub binding: Binding,
    /// Whether asking it reaches a service outside the machine.
    pub remote: bool,
}

/// The models of the project bound to decisions matching `decisions` (ids and
/// globs; all when empty), in a stable order.
pub fn bound_models(session: &Session, decisions: &[String]) -> Result<Vec<BoundModel>> {
    let bound = session.config.models();
    let catalog = session.catalog()?;
    let known: Vec<&str> = catalog.decisions().map(|d| d.id()).collect();
    let selector = Selector::new(decisions)?;
    selector.expand(known.iter().copied())?;
    let remote = |model: &str| {
        lighthouse_spec::ModelBinding::split(model)
            .and_then(|(provider, _)| bound.provider(provider))
            .is_some_and(|provider| provider.remote())
    };
    let mut found = Vec::new();
    if let Some(model) = &bound.classification
        && selector.is_empty()
    {
        found.push(BoundModel {
            model: model.clone(),
            decision: None,
            binding: Binding::Classification,
            remote: remote(model),
        });
    }
    let attached: BTreeMap<String, String> = attached_models(&session.root, &catalog);
    let rules = bound.rules.iter().map(|(d, m)| (d, m, Binding::Rule));
    let trained = attached.iter().map(|(d, m)| (d, m, Binding::Attached));
    for (decision, model, binding) in rules.chain(trained) {
        if selector.matches(decision) {
            found.push(BoundModel {
                model: model.clone(),
                decision: Some(decision.clone()),
                binding,
                remote: remote(model),
            });
        }
    }
    Ok(found)
}

/// Whether the project binds a model that reaches a service outside the
/// machine.
pub fn remote_bound(session: &Session) -> bool {
    bound_models(session, &[]).is_ok_and(|models| models.iter().any(|m| m.remote))
}
