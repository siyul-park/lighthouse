//! The models a project binds to its decisions.

use std::{collections::BTreeMap, path::Path, sync::Arc};

use lighthouse_engine::CommandModels;
use lighthouse_plugin::Classifier;
use lighthouse_rpc::Command;
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
    /// The decision it judges; none for the classification models.
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
    let mut bind = |model: &String, decision: Option<&String>, binding| {
        found.push(BoundModel {
            model: model.clone(),
            decision: decision.cloned(),
            binding,
            remote: remote(model),
        });
    };
    if selector.is_empty() {
        for model in &bound.classification {
            bind(model, None, Binding::Classification);
        }
    }
    let attached: BTreeMap<String, String> = attached_models(&session.root, &catalog);
    for (decision, models) in &bound.rules {
        if selector.matches(decision) {
            for model in models {
                bind(model, Some(decision), Binding::Rule);
            }
        }
    }
    for (decision, model) in &attached {
        if selector.matches(decision) {
            bind(model, Some(decision), Binding::Attached);
        }
    }
    Ok(found)
}

/// Whether the project binds a model that reaches a service outside the
/// machine.
pub fn remote_bound(session: &Session) -> bool {
    bound_models(session, &[]).is_ok_and(|models| models.iter().any(|m| m.remote))
}

/// How the engine runs the `command` model providers of the project at
/// `root`: as programs started from it.
pub(crate) fn command_models(root: &Path) -> CommandModels {
    let root = root.to_owned();
    Arc::new(move |name, model, provider| {
        Command::new(&root, name, model, provider)
            .map(|command| Box::new(command) as Box<dyn Classifier>)
    })
}
