//! What a project says about the models that judge its decisions, and about
//! how a decision's check is evaluated.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{RuleSetting, spec::ProjectSpec};

/// Requests a run may send to models that cost, unless the project says.
pub const DEFAULT_BUDGET: usize = 200;
/// The precision a check must reach unless the project says.
pub const DEFAULT_PRECISION: f64 = 0.9;
/// The share of the answers of an attached model that are sent on, unless the
/// project says.
pub const DEFAULT_EXPLORE: f64 = 0.05;

/// The specs a provider may name, and what each needs by default.
const SPECS: [(&str, &str, &str); 2] = [
    ("openai", "https://api.openai.com/v1", "OPENAI_API_KEY"),
    ("typesafe", "https://api.typesafe.ai", "TYPESAFE_API_KEY"),
];

/// The plugin that speaks the standard APIs.
pub const API_PLUGIN: &str = "model-http";

/// The plugin that learns from a decision's labels, on this machine.
pub const LOCAL_PLUGIN: &str = "model-local";

/// Names a project may not give a provider: they belong to the providers
/// Lighthouse will predefine.
pub const RESERVED: [&str; 2] = ["local", "anthropic"];

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
    /// them (`spec`, `baseUrl`, `keyEnv`, `logprobs`, `extraBody` for `model-http`); the model part of
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
    /// on output, an environment cleared to `PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`,
    /// `TMPDIR`, the declared `env` and the variable `keyEnv` names, and the project's
    /// trust. An answer that does not fit the task's shape is an abstention.
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
        /// Variables the program is given besides the allowlist.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
        /// The variable of the user's environment that holds the program's
        /// key; it is passed through, and no other secret is.
        #[serde(default, rename = "keyEnv", skip_serializing_if = "Option::is_none")]
        key_env: Option<String>,
    },
}

impl ModelProvider {
    /// Whether asking this provider reaches a service outside the machine: a
    /// standard API, or a program that is given an API key.
    pub fn remote(&self) -> bool {
        match self {
            Self::Rpc { plugin, .. } => plugin == API_PLUGIN,
            Self::Command { key_env, .. } => key_env.is_some(),
        }
    }
}

/// A provider that speaks a standard API: Lighthouse knows the format of the
/// requests, and the project says where to send them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ApiProvider {
    /// The API: `openai` (Chat Completions, which OpenRouter, Ollama, vLLM
    /// and LM Studio also speak) or `typesafe` (System One). Others will join.
    pub spec: Names,
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
    /// Read the answer from the probability of the first token (`yes` or
    /// `no`) instead of asking for a number; OpenAI's `logprobs` parameter.
    /// Only for a model whose first token is the answer. Default: false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<bool>,
    /// A JSON object merged shallowly into every request body, its keys
    /// overriding the body's own (the OpenAI SDK's `extra_body`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<Map<String, Value>>,
}

/// One name or several, in the order written: the standard APIs a provider
/// speaks (each operation uses the one that covers it: classification
/// `typesafe`, else `openai`), or the models of a route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Names {
    One(String),
    Many(Vec<String>),
}

impl Names {
    /// The names, in the order written.
    pub fn names(&self) -> Vec<&str> {
        match self {
            Self::One(name) => vec![name.as_str()],
            Self::Many(names) => names.iter().map(String::as_str).collect(),
        }
    }

    fn owned(&self) -> Vec<String> {
        self.names().into_iter().map(str::to_owned).collect()
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
                let names = api.spec.names();
                let (_, base, key) = SPECS
                    .iter()
                    .find(|(spec, ..)| names.contains(spec))
                    .ok_or_else(|| {
                        format!(
                            "none of the specs {names:?} is supported (openai and typesafe are)"
                        )
                    })?;
                let mut options = Map::new();
                options.insert(
                    "spec".to_owned(),
                    serde_json::to_value(&api.spec).unwrap_or_default(),
                );
                options.insert(
                    "baseUrl".to_owned(),
                    Value::from(api.base_url.as_deref().unwrap_or(base)),
                );
                let key_env = api.key_env.as_deref().unwrap_or(key);
                options.insert("keyEnv".to_owned(), Value::from(key_env));
                if let Some(logprobs) = api.logprobs {
                    options.insert("logprobs".to_owned(), Value::from(logprobs));
                }
                if let Some(extra) = &api.extra_body {
                    options.insert("extraBody".to_owned(), Value::Object(extra.clone()));
                }
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
    /// The model (`<provider>/<model>`), or the route of several, that judges
    /// a decision no rule binds and no trained model is attached to. A request
    /// moves to the next model of a route when one fails. When the route
    /// abstains, the subject is left to agent review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Names>,
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
/// `models` table and its rules.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelBinding {
    /// The project's route for decisions no rule binds.
    pub classification: Vec<String>,
    /// The route a rule names for its decision, by decision id.
    pub rules: BTreeMap<String, Vec<String>>,
    /// Which model of a route a request starts at.
    pub routing: Routing,
    /// The providers the project defines, by name.
    pub providers: BTreeMap<String, ModelProvider>,
    /// Providers that cannot work, by name, and why.
    pub invalid: BTreeMap<String, String>,
    /// The evaluation policy a rule sets, by decision id.
    pub evaluation: BTreeMap<String, Evaluation>,
    pub budget: usize,
    /// The share of an attached model's fails that are sent on.
    pub explore: f64,
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
                rules.insert(id.clone(), model.owned());
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
            classification: models
                .classification
                .as_ref()
                .map(Names::owned)
                .unwrap_or_default(),
            rules,
            routing: models.routing.unwrap_or_default(),
            providers,
            invalid,
            evaluation,
            budget: models.budget.unwrap_or(DEFAULT_BUDGET),
            explore: models.explore.unwrap_or(DEFAULT_EXPLORE),
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
            .chain(self.rules.values().flatten())
            .map(String::as_str)
    }

    /// The evaluation policy of a rule: what the project sets, else the default.
    pub fn policy(&self, rule: &str) -> Evaluation {
        self.evaluation.get(rule).copied().unwrap_or_default()
    }
}

/// The providers Lighthouse predefines: the standard APIs, at their own
/// service, and `local`, which costs nothing. Declare a provider of the same
/// name to override an API; `local` is reserved.
pub fn builtin(name: &str) -> Option<ModelProvider> {
    if name == "local" {
        return Some(ModelProvider::Rpc {
            plugin: LOCAL_PLUGIN.to_owned(),
            options: Map::new(),
            cost: Some(0.0),
        });
    }
    SPECS.iter().find(|(spec, ..)| *spec == name)?;
    ProviderSpec::Api(ApiProvider {
        spec: Names::One(name.to_owned()),
        base_url: None,
        key_env: None,
        cost: None,
        logprobs: None,
        extra_body: None,
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
