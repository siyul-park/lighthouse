//! A provider of models: how a project declares one, along two axes (the
//! protocol it speaks, and the reach to it), and what the host runs.

use std::collections::BTreeMap;

use lighthouse_resource::Duration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::models::Names;

/// The protocols a provider may speak over HTTP, and the service and key each
/// has when the project says nothing else.
const PROTOCOLS: [(&str, &str, &str); 2] = [
    ("openai", "https://api.openai.com/v1", "OPENAI_API_KEY"),
    ("typesafe", "https://api.typesafe.ai", "TYPESAFE_API_KEY"),
];

/// The plugin that speaks the standard protocols over HTTP.
const HTTP_PLUGIN: &str = "model-http";

/// The plugin that learns from a decision's labels, on this machine.
const LOCAL_PLUGIN: &str = "model-local";

/// What one answer costs when the provider says nothing else.
const DEFAULT_COST: f64 = 1.0;

/// Names a project may not give a provider: they belong to the providers
/// Lighthouse will predefine.
pub const RESERVED_PROVIDERS: [&str; 2] = ["local", "anthropic"];

/// What a model is asked to do; a binding of `[models]` names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    /// The probability that a subject fails a decision.
    Classification,
    /// Changes to the files of a directory, made by an agent that runs there.
    Edit,
}

impl std::fmt::Display for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Classification => "classification",
            Self::Edit => "edit",
        })
    }
}

/// The protocol a `command` provider speaks over its standard streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandProtocol {
    /// TypeSafe's System One body on stdin, the response on stdout.
    TypeSafe,
    /// The prompt as text on stdin; the program changes the files of the
    /// directory it runs in, and what it prints is kept for the notes only.
    PlainText,
}

/// A provider of models as the project writes it, under
/// `[models.providers.<name>]`. A model is named `<provider>/<model>`.
///
/// A provider is reached by exactly one of `baseUrl` (HTTP), `command` (a
/// stdio process) and `plugin` (an rpc plugin); the only exception is a
/// `protocol` of `openai` or `typesafe` alone, which is HTTP at that
/// protocol's own service. Fields that belong to one reach are refused on the
/// others.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProviderSpec {
    /// The protocol spoken at the reach: `openai` (Chat Completions, which
    /// OpenRouter, Ollama, vLLM and LM Studio also speak) or `typesafe`
    /// (System One), or a list of them in the order of preference. Required
    /// for `baseUrl`. A `command` speaks `typesafe` (it serves classification),
    /// or plain text without a `protocol` (it serves `edit`); a `plugin` must
    /// not name one: its handshake says what it serves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<Names>,
    /// The reach over HTTP: where requests go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The reach as a stdio process under the process contract: no shell, its
    /// own process group, a time limit, a cap on output, an environment
    /// cleared to `PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `TMPDIR`, the
    /// declared `env` and the variable `keyEnv` names, and the project's
    /// trust. `{model}` stands for the model part of the reference. An answer
    /// that does not fit the protocol's shape is no answer. Without a
    /// `protocol` the prompt is plain text on stdin and the program serves
    /// `edit`: it runs in a scratch copy of the project and changes the files
    /// there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// The reach as a listed plugin that offers models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// The environment variable that holds the key: sent as the bearer of an
    /// HTTP request, or passed through to a `command`; none for a service that
    /// needs no key. Default at a protocol's own service: that protocol's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_env: Option<String>,
    /// What one answer costs. Default: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// How long one request may take, such as `90s`: a run of a `command`
    /// (default 120s, 10m for `edit`) or an HTTP request of a `baseUrl`
    /// provider (default 120s; reasoning models may need `10m`). A `plugin`
    /// provider has none: a plugin is not told one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
    /// `command` only: variables the program is given besides the allowlist.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// `command` only: the hosts the program may reach, through the proxy of
    /// the run: a name, or `*.suffix` for every host below it, either with
    /// `:port`. The program runs in the OS sandbox, which reaches nothing
    /// else. Default: none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network: Vec<String>,
    /// `command` with `protocol = "typesafe"` only: RFC 6901 JSON Pointer to
    /// the response inside the printed JSON (stdout is one JSON value), when
    /// the program wraps it in an envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
    /// `plugin` only: what the plugin is told besides the model.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,
    /// HTTP only: a JSON object merged shallowly into every request body, its
    /// keys overriding the body's own (the OpenAI SDK's `extra_body`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<Map<String, Value>>,
}

/// A provider as the host runs it, resolved from what the project wrote.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelProvider {
    /// A plugin process that offers models. The `options` say how to reach
    /// them (`protocol`, `baseUrl`, `keyEnv`, `extraBody` for `model-http`);
    /// the model part of the reference goes with every request.
    Rpc {
        plugin: String,
        options: Map<String, Value>,
        /// What one answer costs, when the plugin's own figure is not right.
        cost: Option<f64>,
    },
    /// A program that reads a request on stdin and prints the answer, in the
    /// shape its `protocol` says.
    Command {
        protocol: CommandProtocol,
        /// The program and its arguments; `{model}` stands for the model part
        /// of the reference.
        argv: Vec<String>,
        pointer: Option<String>,
        cost: f64,
        timeout: Option<Duration>,
        env: BTreeMap<String, String>,
        key_env: Option<String>,
        /// The hosts the program may reach.
        network: Vec<String>,
    },
}

impl ModelProvider {
    /// The providers Lighthouse predefines: the standard protocols, at their
    /// own service, and `local`, which costs nothing. Declare a provider of the
    /// same name to override a protocol; `local` is reserved.
    pub fn predefined(name: &str) -> Option<Self> {
        if name == "local" {
            return Some(Self::Rpc {
                plugin: LOCAL_PLUGIN.to_owned(),
                options: Map::new(),
                cost: Some(0.0),
            });
        }
        PROTOCOLS.iter().find(|(protocol, ..)| *protocol == name)?;
        ProviderSpec {
            protocol: Some(Names::One(name.to_owned())),
            ..ProviderSpec::default()
        }
        .resolve()
        .ok()
    }

    /// Whether its protocol serves `task`; `None` when only the handshake of
    /// the plugin behind it can say. A plugin never serves `edit`.
    pub fn serves(&self, task: Task) -> Option<bool> {
        match self {
            Self::Command { protocol, .. } => Some(match task {
                Task::Classification => *protocol == CommandProtocol::TypeSafe,
                // Running the same program in a scratch copy is what makes it
                // an edit, which only plain text allows.
                Task::Edit => *protocol == CommandProtocol::PlainText,
            }),
            Self::Rpc {
                plugin, options, ..
            } if plugin == HTTP_PLUGIN => {
                let speaks = |name: &str| match options.get("protocol") {
                    Some(Value::String(protocol)) => protocol == name,
                    Some(Value::Array(list)) => list.iter().any(|p| p.as_str() == Some(name)),
                    _ => false,
                };
                Some(match task {
                    Task::Classification => speaks("typesafe") || speaks("openai"),
                    Task::Edit => false,
                })
            }
            Self::Rpc { .. } => (task == Task::Edit).then_some(false),
        }
    }

    /// Whether asking this provider reaches a service outside the machine: a
    /// standard protocol over HTTP, or a program that is given a key.
    pub fn remote(&self) -> bool {
        match self {
            Self::Rpc { plugin, .. } => plugin == HTTP_PLUGIN,
            Self::Command { key_env, .. } => key_env.is_some(),
        }
    }
}

impl ProviderSpec {
    /// The provider this is, or why it cannot be.
    pub fn resolve(&self) -> Result<ModelProvider, String> {
        let reaches = given(&[
            ("baseUrl", self.base_url.is_some()),
            ("command", self.command.is_some()),
            ("plugin", self.plugin.is_some()),
        ]);
        if reaches.len() > 1 {
            return Err(format!(
                "{} are alternatives: a provider is reached by exactly one of `baseUrl`, `command` and `plugin`",
                reaches.join(" and ")
            ));
        }
        match (&self.command, &self.plugin) {
            (Some(argv), _) => self.command_provider(argv),
            (_, Some(plugin)) => self.plugin_provider(plugin),
            _ => self.http_provider(),
        }
    }

    /// A provider reached over HTTP, served by `model-http`.
    fn http_provider(&self) -> Result<ModelProvider, String> {
        only("a `baseUrl` provider", &self.command_only())?;
        only(
            "a `baseUrl` provider",
            &[("options", !self.options.is_empty())],
        )?;
        let Some(protocol) = &self.protocol else {
            return Err(match self.base_url {
                Some(_) => "`baseUrl` needs a `protocol`: the one the service speaks (openai or typesafe)".to_owned(),
                None => "a provider needs `baseUrl`, `command` or `plugin`; only a `protocol` of openai or typesafe alone reaches that protocol's own service".to_owned(),
            });
        };
        let names: Vec<&str> = protocol.iter().collect();
        let (_, own, key) = PROTOCOLS
            .iter()
            .find(|(protocol, ..)| names.contains(protocol))
            .ok_or_else(|| {
                format!("none of the protocols {names:?} is supported (openai and typesafe are)")
            })?;
        let (base_url, key_env) = match &self.base_url {
            Some(base_url) => (base_url.as_str(), self.key_env.clone()),
            None if names.len() == 1 => (
                *own,
                Some(self.key_env.as_deref().unwrap_or(key).to_owned()),
            ),
            None => {
                return Err(format!(
                    "`protocol` names several of {names:?}, which have no service of their own: add a `baseUrl`"
                ));
            }
        };
        let mut options = Map::new();
        options.insert(
            "protocol".to_owned(),
            serde_json::to_value(protocol).unwrap_or_default(),
        );
        options.insert("baseUrl".to_owned(), Value::from(base_url));
        if let Some(key_env) = key_env {
            options.insert("keyEnv".to_owned(), Value::from(key_env));
        }
        if let Some(extra) = &self.extra_body {
            options.insert("extraBody".to_owned(), Value::Object(extra.clone()));
        }
        if let Some(timeout) = self.timeout {
            options.insert("timeout".to_owned(), Value::from(timeout.to_string()));
        }
        Ok(ModelProvider::Rpc {
            plugin: HTTP_PLUGIN.to_owned(),
            options,
            cost: self.cost,
        })
    }

    /// A provider that is a program speaking TypeSafe's System One.
    fn command_provider(&self, argv: &[String]) -> Result<ModelProvider, String> {
        only(
            "a `command` provider",
            &[("extraBody", self.extra_body.is_some())],
        )?;
        only(
            "a `command` provider",
            &[("options", !self.options.is_empty())],
        )?;
        if argv.is_empty() {
            return Err("`command` is empty".to_owned());
        }
        let protocol = match self.protocol.as_ref().map(|p| p.iter().collect::<Vec<_>>()) {
            None => {
                if self.pointer.is_some() {
                    return Err(
                        "a `command` without a `protocol` has no `pointer`: only `typesafe` reads a response".to_owned(),
                    );
                }
                CommandProtocol::PlainText
            }
            Some(names) if names == ["typesafe"] => CommandProtocol::TypeSafe,
            Some(names) => {
                return Err(format!(
                    "a `command` speaks `typesafe` (classification) or plain text (edit), not {names:?}"
                ));
            }
        };
        Ok(ModelProvider::Command {
            protocol,
            argv: argv.to_vec(),
            pointer: self.pointer.clone(),
            cost: self.cost.unwrap_or(DEFAULT_COST),
            timeout: self.timeout,
            env: self.env.clone(),
            key_env: self.key_env.clone(),
            network: self.network.clone(),
        })
    }

    /// A provider that is a plugin the project lists.
    fn plugin_provider(&self, plugin: &str) -> Result<ModelProvider, String> {
        only("a `plugin` provider", &self.command_only())?;
        only(
            "a `plugin` provider",
            &[
                ("timeout", self.timeout.is_some()),
                ("protocol", self.protocol.is_some()),
                ("keyEnv", self.key_env.is_some()),
                ("extraBody", self.extra_body.is_some()),
            ],
        )?;
        Ok(ModelProvider::Rpc {
            plugin: plugin.to_owned(),
            options: self.options.clone(),
            cost: self.cost,
        })
    }

    /// The fields only a `command` has, and whether each is given.
    fn command_only(&self) -> [(&'static str, bool); 3] {
        [
            ("env", !self.env.is_empty()),
            ("pointer", self.pointer.is_some()),
            ("network", !self.network.is_empty()),
        ]
    }
}

/// An error naming the fields of `strays` that are given, which `what` has
/// not.
fn only(what: &str, strays: &[(&str, bool)]) -> Result<(), String> {
    let strays = given(strays);
    if strays.is_empty() {
        Ok(())
    } else {
        Err(format!("{what} has no {}", strays.join(", ")))
    }
}

/// The names, in code font, of the fields that are given.
fn given(fields: &[(&str, bool)]) -> Vec<String> {
    fields
        .iter()
        .filter(|(_, given)| *given)
        .map(|(field, _)| format!("`{field}`"))
        .collect()
}
