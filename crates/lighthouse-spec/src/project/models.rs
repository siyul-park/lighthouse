//! What a project says about the models that judge its decisions, and about
//! how a decision's check is evaluated.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{RuleSetting, spec::ProjectSpec};

/// Calls a run may spend on models unless the project says.
pub const DEFAULT_BUDGET: usize = 200;
/// Subjects sent to a model in one request unless the project says.
pub const DEFAULT_BATCH: usize = 20;
/// The precision a check must reach unless the project says.
pub const DEFAULT_PRECISION: f64 = 0.9;

/// The specs a provider may name, and what each needs by default.
const SPECS: [(&str, &str, &str); 1] = [("openai", "https://api.openai.com/v1", "OPENAI_API_KEY")];

/// The plugin that speaks the standard APIs.
pub const API_PLUGIN: &str = "model-http";

/// Names a project may not give a provider: they belong to the providers
/// Lighthouse will predefine.
pub const RESERVED: [&str; 3] = ["local", "typesafe", "anthropic"];

/// What a `command` model is given on stdin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ModelInput {
    /// The request as JSON, as a model plugin gets it.
    #[default]
    Request,
    /// The request as text for a language model, with the answer's shape.
    Prompt,
}

/// How a `command` model prints its answer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ModelOutput {
    /// stdout is one JSON value.
    #[default]
    Json,
}

/// A provider of models, named under `[models.providers.<name>]`: a provider,
/// as a check or a fix is. A model is named `<provider>/<model>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ModelProvider {
    /// A plugin process that offers models. The `options` say how to reach
    /// them (`format`, `baseUrl`, `keyEnv` for `model-http`); the model part of
    /// the reference goes with every request.
    Rpc {
        /// A listed plugin that offers models.
        plugin: String,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        options: Map<String, Value>,
        /// What one answer costs, when the plugin's own figure is not right.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost: Option<f64>,
    },
    /// Any program that reads a request and prints an answer, under the
    /// process contract: no shell, its own process group, a time limit, a cap
    /// on output, and the project's trust. An answer that does not fit the
    /// task's shape is an abstention.
    Command {
        /// The program and its arguments; `{model}` stands for the model part
        /// of the reference.
        argv: Vec<String>,
        #[serde(default)]
        stdin: ModelInput,
        #[serde(default)]
        output: ModelOutput,
        /// RFC 6901 JSON Pointer to the answer inside the printed JSON, when
        /// the program wraps it in an envelope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pointer: Option<String>,
        /// Default: `classification`.
        #[serde(default = "default_tasks")]
        tasks: Vec<String>,
        /// What one answer costs. Default: 1.
        #[serde(default = "default_cost")]
        cost: f64,
        /// How long one run may take, such as `90s`. Default: 120s.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<String>,
    },
}

/// A provider that speaks a standard API: Lighthouse knows the format of the
/// requests, and the project says where to send them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiProvider {
    /// The API: `openai` (Chat Completions, which OpenRouter, Ollama, vLLM
    /// and LM Studio also speak). Others will join.
    pub spec: String,
    /// Where requests go. Default: the spec's own service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The environment variable that holds the API key; none for a service
    /// that needs no key. Default: the spec's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_env: Option<String>,
    /// What one answer costs. Default: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

/// A provider as the project writes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ProviderSpec {
    Api(ApiProvider),
    Provider(ModelProvider),
}

impl ProviderSpec {
    /// The provider this is, or why it cannot be.
    pub fn resolve(&self) -> Result<ModelProvider, String> {
        match self {
            Self::Provider(provider) => Ok(provider.clone()),
            Self::Api(api) => {
                let (_, base, key) = SPECS
                    .iter()
                    .find(|(spec, ..)| *spec == api.spec)
                    .ok_or_else(|| format!("spec `{}` is not supported (openai is)", api.spec))?;
                let mut options = Map::new();
                options.insert("spec".to_owned(), Value::from(api.spec.as_str()));
                options.insert(
                    "baseUrl".to_owned(),
                    Value::from(api.base_url.as_deref().unwrap_or(base)),
                );
                let key_env = api.key_env.as_deref().unwrap_or(key);
                options.insert("keyEnv".to_owned(), Value::from(key_env));
                Ok(ModelProvider::Rpc {
                    plugin: API_PLUGIN.to_owned(),
                    options,
                    cost: api.cost,
                })
            }
        }
    }
}

/// The `models` table of a project: its bindings and limits, and its own
/// providers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelsSpec {
    /// The model (`<provider>/<model>`) that judges a decision no rule binds
    /// and no trained model is attached to. When it abstains, the subject is
    /// left to agent review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<String>,
    /// Subjects a run may put to models that cost something; the rest are
    /// left to review. Default: 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<usize>,
    /// Subjects in one request to a model. Default: 20.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<usize>,
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

/// Which error an evaluation tolerates when it cannot have both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Tolerate {
    /// Missed violations: precision is what counts.
    #[default]
    Fn,
    /// False alarms: recall is what counts.
    Fp,
}

/// How a rule's check is judged by `lighthouse eval`. A policy, not a
/// meaning: changing it moves no meaning version.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Evaluation {
    /// The precision a check must reach, from 0 to 1. Default: 0.9.
    #[serde(default = "default_precision")]
    pub precision: f64,
    /// Default: `fn`.
    #[serde(default)]
    pub tolerate: Tolerate,
}

impl Default for Evaluation {
    fn default() -> Self {
        Self {
            precision: DEFAULT_PRECISION,
            tolerate: Tolerate::default(),
        }
    }
}

/// The models of a project and the evaluation policies, resolved from its
/// `models` table and its rules.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelBinding {
    /// The project's model for decisions no rule binds.
    pub classification: Option<String>,
    /// The instance a rule names for its decision, by decision id.
    pub rules: BTreeMap<String, String>,
    /// The providers the project defines, by name.
    pub providers: BTreeMap<String, ModelProvider>,
    /// Providers that cannot work, by name, and why.
    pub invalid: BTreeMap<String, String>,
    /// The evaluation policy a rule sets, by decision id.
    pub evaluation: BTreeMap<String, Evaluation>,
    pub budget: usize,
    pub batch: usize,
}

impl ModelBinding {
    pub(super) fn of(spec: &ProjectSpec) -> Self {
        let details = spec.rules.iter().filter_map(|(id, setting)| match setting {
            RuleSetting::Detailed(detail) => Some((id, detail)),
            RuleSetting::Level(_) => None,
        });
        let mut rules = BTreeMap::new();
        let mut evaluation = BTreeMap::new();
        for (id, detail) in details {
            if let Some(model) = &detail.model {
                rules.insert(id.clone(), model.clone());
            }
            if let Some(policy) = detail.evaluation {
                evaluation.insert(id.clone(), policy);
            }
        }
        let models = &spec.models;
        let mut providers = BTreeMap::new();
        let mut invalid = BTreeMap::new();
        for (name, provider) in &models.providers {
            match provider.resolve() {
                Ok(provider) => providers.insert(name.clone(), provider).map(drop),
                Err(reason) => invalid.insert(name.clone(), reason).map(drop),
            };
        }
        Self {
            classification: models.classification.clone(),
            rules,
            providers,
            invalid,
            evaluation,
            budget: models.budget.unwrap_or(DEFAULT_BUDGET),
            batch: models.batch.unwrap_or(DEFAULT_BATCH).max(1),
        }
    }

    /// The provider and the model of a reference `<provider>/<model>`, split
    /// at the first `/`.
    pub fn split(reference: &str) -> Option<(&str, &str)> {
        reference
            .split_once('/')
            .filter(|(provider, model)| !provider.is_empty() && !model.is_empty())
    }

    /// The provider of that name: the project's own, else a preset.
    pub fn provider(&self, name: &str) -> Option<ModelProvider> {
        self.providers.get(name).cloned().or_else(|| builtin(name))
    }

    /// Every model reference the project binds.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.classification
            .iter()
            .chain(self.rules.values())
            .map(String::as_str)
    }

    /// The evaluation policy of a rule: what the project sets, else the default.
    pub fn policy(&self, rule: &str) -> Evaluation {
        self.evaluation.get(rule).copied().unwrap_or_default()
    }
}

/// The providers Lighthouse predefines: the standard APIs, at their own
/// service. Declare a provider of the same name to override one.
pub fn builtin(name: &str) -> Option<ModelProvider> {
    SPECS.iter().find(|(spec, ..)| *spec == name)?;
    ProviderSpec::Api(ApiProvider {
        spec: name.to_owned(),
        base_url: None,
        key_env: None,
        cost: None,
    })
    .resolve()
    .ok()
}

fn default_tasks() -> Vec<String> {
    vec!["classification".to_owned()]
}

fn default_cost() -> f64 {
    1.0
}

fn default_precision() -> f64 {
    DEFAULT_PRECISION
}
