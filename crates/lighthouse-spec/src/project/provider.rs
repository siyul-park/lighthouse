//! A provider of models: how a project declares one, along two axes (the
//! protocol it speaks, and the reach to it), and what the host runs.

use std::{collections::BTreeMap, net::IpAddr};

use lighthouse_process::{Policy, variables};
use lighthouse_resource::Duration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{events::EventsSpec, models::Names};
use crate::{Point, Program, Sandbox};

/// The protocols a provider may speak over HTTP, and the service and key each
/// has when the project says nothing else.
const PROTOCOLS: [(&str, &str, &str); 2] = [
    ("openai", "https://api.openai.com/v1", "OPENAI_API_KEY"),
    ("typesafe", "https://api.typesafe.ai", "TYPESAFE_API_KEY"),
];

/// The header that carries a key.
const AUTHORIZATION: &str = "Authorization";

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
    Agent,
}

impl std::fmt::Display for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Classification => "classification",
            Self::Agent => "agent",
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
/// A provider is reached by exactly one of `url` (HTTP), `command` (a stdio
/// process) and `plugin` (an rpc plugin); the only exception is a `protocol`
/// of `openai` or `typesafe` alone, which is HTTP at that protocol's own
/// service. Fields that belong to one reach are refused on the others.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderSpec {
    /// The protocol spoken at the reach: `openai` (Chat Completions, which
    /// OpenRouter, Ollama, vLLM and LM Studio also speak) or `typesafe`
    /// (System One), or a list of them in the order of preference. Required
    /// for `url`. A `command` speaks `typesafe` (it serves classification),
    /// or plain text without a `protocol` (it serves `agent`); a `plugin` must
    /// not name one: its handshake says what it serves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<Names>,
    /// The reach over HTTP: where requests go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The reach as a stdio process under the process contract of a program
    /// (see `docs/programs.md`): no shell, its own process group, a time
    /// limit, a cap on output, the one environment plus the declared `env`,
    /// and the project's trust. `{model}` stands for the model part of the
    /// reference. An answer that does not fit the protocol's shape is no
    /// answer. Without a `protocol` the prompt is plain text on stdin and the
    /// program serves `agent`: it runs in a scratch copy of the project and
    /// changes the files there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// The reach as a listed plugin that offers models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// HTTP only: headers sent with every request. A value may hold
    /// `${NAME}`, the user's variable `NAME`, which is never written into a
    /// note, a log or a cache key; a variable that is not set fails the
    /// request, naming it. Default at a protocol's own service:
    /// `Authorization: Bearer ${OPENAI_API_KEY}` (`${TYPESAFE_API_KEY}`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// What one answer costs. Default: 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// How long one request may take, such as `90s`: a run of a `command`
    /// (default 2m, 10m for `agent`) or an HTTP request of a `url` provider
    /// (default 2m; reasoning models may need `10m`). A `plugin` provider has
    /// none: a plugin is not told one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
    /// `command` only: variables the program is given besides the allowlist;
    /// a value may hold `${NAME}` (see `headers`), which is how a key reaches
    /// the program.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// `command` only: what the program may reach besides the project's
    /// `[sandbox]`. A provider whose `sandbox.network` lists a host is remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<Sandbox>,
    /// `command` with `protocol = "typesafe"` only: RFC 6901 JSON Pointer to
    /// the response inside the printed JSON (stdout is one JSON value), when
    /// the program wraps it in an envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
    /// `command` without a `protocol` only: how the JSON lines of the
    /// agent's stdout become the tool events of the fix report, in CEL (see
    /// `EventsSpec`). Without it the report holds no events of this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<EventsSpec>,
    /// `plugin` only: what the plugin is told besides the model.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,
    /// HTTP only: a JSON object merged shallowly into every request body, its
    /// keys overriding the body's own (the OpenAI SDK's `extra_body`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Map<String, Value>>,
}

/// A provider as the host runs it, resolved from what the project wrote.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelProvider {
    /// A plugin process that offers models. The `options` say how to reach
    /// them (`protocol`, `url`, `headers`, `body` for `model-http`); the model
    /// part of the reference goes with every request.
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
        /// The program: its `command` has `{model}` for the model part of the
        /// reference.
        program: Program,
        pointer: Option<String>,
        /// What an agent's stdout means, when the project says.
        events: Option<Box<EventsSpec>>,
        cost: f64,
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
    /// the plugin behind it can say.
    pub fn serves(&self, task: Task) -> Option<bool> {
        match self {
            Self::Command { protocol, .. } => Some(match task {
                Task::Classification => *protocol == CommandProtocol::TypeSafe,
                // Running the same program in a scratch copy is what makes it
                // an agent, which only plain text allows.
                Task::Agent => *protocol == CommandProtocol::PlainText,
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
                    Task::Agent => false,
                })
            }
            Self::Rpc { .. } => None,
        }
    }

    /// Whether asking this provider reaches a service outside the machine:
    /// HTTP at a `url` that is not loopback, or a program that may reach a
    /// host, by its own `sandbox` or by the `project` policy it runs under.
    /// What leaves the machine is what reaches a host.
    pub fn remote(&self, project: &Policy) -> bool {
        match self {
            Self::Rpc {
                plugin, options, ..
            } => {
                plugin == HTTP_PLUGIN
                    && !options
                        .get("url")
                        .and_then(Value::as_str)
                        .is_some_and(loopback)
            }
            Self::Command { program, .. } => !program.policy(project).network.is_empty(),
        }
    }
}

impl ProviderSpec {
    /// The provider this is, or why it cannot be.
    pub fn resolve(&self) -> Result<ModelProvider, String> {
        let reaches = given(&[
            ("url", self.url.is_some()),
            ("command", self.command.is_some()),
            ("plugin", self.plugin.is_some()),
        ]);
        if reaches.len() > 1 {
            return Err(format!(
                "{} are alternatives: a provider is reached by exactly one of `url`, `command` and `plugin`",
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
        only("a `url` provider", &self.command_only())?;
        only("a `url` provider", &[("options", !self.options.is_empty())])?;
        let Some(protocol) = &self.protocol else {
            return Err(match self.url {
                Some(_) => "`url` needs a `protocol`: the one the service speaks (openai or typesafe)".to_owned(),
                None => "a provider needs `url`, `command` or `plugin`; only a `protocol` of openai or typesafe alone reaches that protocol's own service".to_owned(),
            });
        };
        let names: Vec<&str> = protocol.iter().collect();
        let (_, own, key) = PROTOCOLS
            .iter()
            .find(|(protocol, ..)| names.contains(protocol))
            .ok_or_else(|| {
                format!("none of the protocols {names:?} is supported (openai and typesafe are)")
            })?;
        let mut headers = self.headers.clone();
        let url = match &self.url {
            Some(url) => url.as_str(),
            None if names.len() == 1 => {
                // The key of the protocol's own service, unless the project
                // sends its own.
                if !headers
                    .keys()
                    .any(|h| h.eq_ignore_ascii_case(AUTHORIZATION))
                {
                    headers.insert(AUTHORIZATION.to_owned(), format!("Bearer ${{{key}}}"));
                }
                *own
            }
            None => {
                return Err(format!(
                    "`protocol` names several of {names:?}, which have no service of their own: add a `url`"
                ));
            }
        };
        for (name, value) in &headers {
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_graphic() && b != b':') {
                return Err(format!(
                    "`headers` has the name `{name}`, which is not a header"
                ));
            }
            variables(value).map_err(|why| format!("`headers.{name}`: {why}"))?;
        }
        let mut options = Map::new();
        options.insert(
            "protocol".to_owned(),
            serde_json::to_value(protocol).unwrap_or_default(),
        );
        options.insert("url".to_owned(), Value::from(url));
        if !headers.is_empty() {
            options.insert(
                "headers".to_owned(),
                serde_json::to_value(&headers).unwrap_or_default(),
            );
        }
        if let Some(body) = &self.body {
            options.insert("body".to_owned(), Value::Object(body.clone()));
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
            &[
                ("headers", !self.headers.is_empty()),
                ("body", self.body.is_some()),
                ("options", !self.options.is_empty()),
            ],
        )?;
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
                    "a `command` speaks `typesafe` (classification) or plain text (agent), not {names:?}"
                ));
            }
        };
        if let Some(events) = &self.events {
            if protocol != CommandProtocol::PlainText {
                return Err(
                    "only a `command` without a `protocol`, an agent, has `events`".to_owned(),
                );
            }
            if let Some(why) = events.problem() {
                return Err(why);
            }
        }
        let point = match protocol {
            CommandProtocol::TypeSafe => Point::Provider,
            CommandProtocol::PlainText => Point::Agent,
        };
        let program = Program {
            command: argv.to_vec(),
            env: self.env.clone(),
            timeout: self.timeout,
            sandbox: self.sandbox.clone(),
            ..Program::of(Vec::new())
        };
        if let Some(why) = program.problem(point) {
            return Err(why);
        }
        Ok(ModelProvider::Command {
            protocol,
            program,
            pointer: self.pointer.clone(),
            events: self.events.clone().map(Box::new),
            cost: self.cost.unwrap_or(DEFAULT_COST),
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
                ("headers", !self.headers.is_empty()),
                ("body", self.body.is_some()),
            ],
        )?;
        Ok(ModelProvider::Rpc {
            plugin: plugin.to_owned(),
            options: self.options.clone(),
            cost: self.cost,
        })
    }

    /// The fields only a `command` has, and whether each is given.
    fn command_only(&self) -> [(&'static str, bool); 4] {
        [
            ("env", !self.env.is_empty()),
            ("pointer", self.pointer.is_some()),
            ("events", self.events.is_some()),
            ("sandbox", self.sandbox.is_some()),
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

/// Whether `url` names this machine: its host is exactly `localhost` or an IP
/// address that is loopback (`127.0.0.0/8`, `::1`). A name that only starts
/// like one (`127.attacker.example`) is not.
fn loopback(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host.strip_prefix('[') {
        Some(inside) => inside.split(']').next().unwrap_or_default(),
        None => host.rsplit_once(':').map_or(host, |(name, _)| name),
    };
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}
