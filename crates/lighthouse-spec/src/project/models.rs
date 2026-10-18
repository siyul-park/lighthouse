//! What a project says about the models that judge its decisions, and about
//! how a decision's check is evaluated.

use std::{collections::BTreeMap, sync::Arc};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ModelProvider, ProviderSpec, Rules, Task};

/// Requests a run may send to models that cost, unless the project says.
pub(super) const DEFAULT_BUDGET: usize = 200;
/// The precision a check must reach unless the project says.
const DEFAULT_PRECISION: f64 = 0.9;
/// The share of the answers of an attached model that are sent on, unless the
/// project says.
pub(super) const DEFAULT_EXPLORE: f64 = 0.05;

/// One name or several, in the order written: the protocols a provider speaks
/// (each operation uses the one that covers it: classification `typesafe`,
/// else `openai`), or the models of a route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Names {
    One(String),
    Many(Vec<String>),
}

impl Names {
    /// The names, in the order written.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        let names = match self {
            Self::One(name) => std::slice::from_ref(name),
            Self::Many(names) => names.as_slice(),
        };
        names.iter().map(String::as_str)
    }
}

/// Which model of a route a request goes to first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Routing {
    /// The first model, then the others in the order written.
    #[default]
    Ordered,
    /// Each request starts at the next model, so load is spread; the others
    /// follow in the order written, wrapping.
    RoundRobin,
}

/// The `models` table of a project: its bindings and limits, and its own
/// providers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelsSpec {
    /// The model (`<provider>/<model>`), or the route of several, that judges
    /// a decision no rule binds and no trained model is attached to. A request
    /// moves to the next model of a route when one fails. When the route
    /// abstains, the subject is left to agent review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Names>,
    /// The model (`<provider>/<model>`), or the route of several, that fixes
    /// by changing files: a headless agent run as a `command` without a
    /// `protocol`, in a scratch copy of the project. It makes the fixes of
    /// the decisions `[fix] decisions` lists, unless a rule binds its own
    /// `edit`. A request moves to the next model of a route when one fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<Names>,
    /// Which model of a route a request starts at. Default: `ordered`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<Routing>,
    /// Requests a run may send to models that cost something; the subjects
    /// of the rest are left to review. Default: 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<usize>,
    /// The share, from 0 to 1, of the subjects an attached model fails that are
    /// sent on to the next layer as if it had abstained, so that labels keep
    /// coming from where the model is surest. The same subjects every run.
    /// Default: 0.05.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explore: Option<f64>,
    /// The providers the project defines, by name; a name that is built in
    /// overrides it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderSpec>,
}

impl ModelsSpec {
    /// Whether the project says nothing about models.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// How a rule's check is judged by `lighthouse eval`. A policy, not a
/// meaning: changing it moves no meaning version.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Evaluation {
    /// The precision a check must reach, from 0 to 1. Default: 0.9.
    #[serde(default = "default_precision")]
    pub precision: f64,
    /// The recall a check must reach, from 0 to 1; no target when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recall: Option<f64>,
}

impl Default for Evaluation {
    fn default() -> Self {
        Self {
            precision: DEFAULT_PRECISION,
            recall: None,
        }
    }
}

/// The models of a project and the evaluation policies, resolved from its
/// `models` table and the rules it and the projects it extends set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Models {
    /// The project's route for decisions no rule binds.
    pub classification: Vec<String>,
    /// The project's route for the agent that fixes by editing files.
    pub edit: Vec<String>,
    /// Which model of a route a request starts at.
    pub routing: Routing,
    /// The providers the project defines, by name.
    pub providers: BTreeMap<String, ModelProvider>,
    /// Providers that cannot work, by name, and why.
    pub invalid: BTreeMap<String, String>,
    pub budget: usize,
    /// The share of an attached model's fails that are sent on.
    pub explore: f64,
    /// The rules, from which each decision's route and evaluation policy come.
    rules: Arc<Rules>,
}

impl Models {
    pub(super) fn new(models: &ModelsSpec) -> Self {
        let mut providers = BTreeMap::new();

        let mut invalid = BTreeMap::new();
        for (name, provider) in &models.providers {
            match provider.resolve() {
                Ok(provider) => providers.insert(name.clone(), provider).map(drop),
                Err(reason) => invalid.insert(name.clone(), reason).map(drop),
            };
        }
        Self {
            classification: models
                .classification
                .as_ref()
                .map(|names| names.iter().map(str::to_owned).collect())
                .unwrap_or_default(),
            edit: models
                .edit
                .as_ref()
                .map(|names| names.iter().map(str::to_owned).collect())
                .unwrap_or_default(),
            routing: models.routing.unwrap_or_default(),
            providers,
            invalid,
            budget: models.budget.unwrap_or(DEFAULT_BUDGET),
            explore: models.explore.unwrap_or(DEFAULT_EXPLORE),
            rules: Arc::default(),
        }
    }

    /// These models with the routes and policies of `rules`.
    pub(super) fn with_rules(&self, rules: Arc<Rules>) -> Self {
        Self {
            rules,
            ..self.clone()
        }
    }

    /// The provider of that name: the project's own, else a predefined one.
    pub fn provider(&self, name: &str) -> Option<ModelProvider> {
        self.providers
            .get(name)
            .cloned()
            .or_else(|| ModelProvider::predefined(name))
    }

    /// The route a rule names for its decision.
    pub fn route(&self, rule: &str) -> Option<&[String]> {
        self.rules.get(rule)?.classification.as_deref()
    }

    /// The route of the agent that fixes `rule` by editing files: what the
    /// rule names, else the project's `edit`. Empty when no agent is bound.
    pub fn edit_for(&self, rule: &str) -> &[String] {
        self.rules
            .get(rule)
            .and_then(|r| r.edit.as_deref())
            .unwrap_or(&self.edit)
    }

    /// Whether any agent that edits is bound, to the project or to a rule.
    pub fn binds_edit(&self) -> bool {
        self.bindings().any(|(task, _)| task == Task::Edit)
    }

    /// The routes rules name, by decision id.
    pub fn routes(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.rules
            .iter()
            .filter_map(|(id, rule)| Some((id.as_str(), rule.classification.as_deref()?)))
    }

    /// The routes of the agents rules name, by decision id.
    pub fn edit_routes(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.rules
            .iter()
            .filter_map(|(id, rule)| Some((id.as_str(), rule.edit.as_deref()?)))
    }

    /// Every model reference the project binds, with the task it binds it to.
    pub fn bindings(&self) -> impl Iterator<Item = (Task, &str)> {
        let classification = self.classification.iter().chain(
            self.rules
                .values()
                .filter_map(|r| r.classification.as_ref())
                .flatten(),
        );
        let edit = self.edit.iter().chain(
            self.rules
                .values()
                .filter_map(|r| r.edit.as_ref())
                .flatten(),
        );
        classification
            .map(|id| (Task::Classification, id.as_str()))
            .chain(edit.map(|id| (Task::Edit, id.as_str())))
    }

    /// The evaluation policy of a rule: what the project sets, else the default.
    pub fn policy(&self, rule: &str) -> Evaluation {
        self.rules
            .get(rule)
            .and_then(|r| r.evaluation)
            .unwrap_or_default()
    }
}

fn default_precision() -> f64 {
    DEFAULT_PRECISION
}
