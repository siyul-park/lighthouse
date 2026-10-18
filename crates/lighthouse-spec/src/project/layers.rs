//! The layers a project's environment is read from, in order: the bundled
//! defaults, the project file, the user's file and the repository-local file.
//! Meaning is the project's alone (see [`EnvironmentSpec`]); the layers after
//! it only change how this machine computes and applies.

use std::collections::BTreeMap;

use lighthouse_resource::{Format, Resource};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::{
    Config, ProviderSpec, Routing,
    environment::EnvironmentSpec,
    fixing::FixEnvironment,
    models::{DEFAULT_BUDGET, DEFAULT_EXPLORE, ModelsSpec},
    parse_project,
    sandbox::SandboxSpec,
};
use crate::Error;

/// What the value of a variable a provider passes to its program is shown as.
const HIDDEN: &str = "***";

/// Where the defaults come from, in the entries.
pub const BUNDLED: &str = "bundled";

/// One file of the personal layers, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// The file it was read from, as shown to the user.
    pub origin: String,
    pub spec: EnvironmentSpec,
}

impl Layer {
    /// The layer written in `text`, the content of the file `origin`.
    pub fn parse(origin: &str, text: &str) -> Result<Self, Error> {
        let spec = EnvironmentSpec::parse(origin, text)?;
        spec.sandbox.check(origin)?;
        Ok(Self {
            origin: origin.to_owned(),
            spec,
        })
    }
}

/// An environment key with its effective value, and the layer it came from.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Entry {
    /// The key, as a TOML dotted key: `models.agent`,
    /// `rules."design/complexity".agent`.
    pub key: String,
    /// The value as the configuration writes it; what may carry a secret in
    /// a provider is hidden.
    pub value: Value,
    /// `bundled`, or the file that set it.
    pub origin: String,
}

/// The layers of one project's environment: the project file first, then the
/// user's and the repository-local file.
#[derive(Debug, Clone, Default)]
pub struct Layers {
    stack: Vec<Layer>,
}

impl Layers {
    /// The configuration of the project document `text`, read from the file
    /// `path`, with the environment `files` over it, the last winning, and the
    /// layers it was made of.
    pub fn build(
        format: Format,
        path: &str,
        text: &str,
        files: Vec<Layer>,
    ) -> Result<(Config, Self), Error> {
        let project = parse_project(format, path, text)?;
        let own = EnvironmentSpec::of_project(&project.spec);
        own.sandbox.check(path)?;
        let mut personal = EnvironmentSpec::default();
        for layer in &files {
            personal.over(&layer.spec);
        }
        let mut merged = own.clone();
        merged.over(&personal);
        let mut spec = project.spec;
        spec.models = merged.models;
        spec.fix.unsafe_fixes = merged.fix.unsafe_fixes;
        spec.sandbox = merged.sandbox;
        let mut config = Config::from_resource(Resource::new(project.metadata, spec))?;
        config.environment = personal.rules;
        let project = Layer {
            origin: path.to_owned(),
            spec: own,
        };
        let stack = std::iter::once(project).chain(files).collect();
        Ok((config, Self { stack }))
    }

    /// Every environment key that has a value, sorted by key, with the layer
    /// the winning value is from.
    pub fn entries(&self) -> Vec<Entry> {
        let mut found: BTreeMap<String, Entry> = BTreeMap::new();
        for layer in std::iter::once(&defaults()).chain(&self.stack) {
            for (key, value) in flatten(&layer.spec) {
                let origin = layer.origin.clone();
                found.insert(key.clone(), Entry { key, value, origin });
            }
        }
        found.into_values().collect()
    }
}

/// What a machine has when no layer says otherwise.
fn defaults() -> Layer {
    Layer {
        origin: BUNDLED.to_owned(),
        spec: EnvironmentSpec {
            models: ModelsSpec {
                routing: Some(Routing::default()),
                budget: Some(DEFAULT_BUDGET),
                explore: Some(DEFAULT_EXPLORE),
                ..ModelsSpec::default()
            },
            fix: FixEnvironment {
                unsafe_fixes: Some(false),
            },
            sandbox: SandboxSpec {
                enabled: Some(true),
                network: Some(Vec::new()),
                write: Some(Vec::new()),
            },
            ..EnvironmentSpec::default()
        },
    }
}

/// The keys one layer sets, with their values.
fn flatten(env: &EnvironmentSpec) -> Vec<(String, Value)> {
    let models = &env.models;
    let mut out: Vec<(String, Option<Value>)> = vec![
        (
            "fix.unsafe".to_owned(),
            env.fix.unsafe_fixes.and_then(to_value),
        ),
        ("models.budget".to_owned(), models.budget.and_then(to_value)),
        (
            "models.classification".to_owned(),
            models.classification.as_ref().and_then(to_value),
        ),
        (
            "models.agent".to_owned(),
            models.agent.as_ref().and_then(to_value),
        ),
        (
            "models.explore".to_owned(),
            models.explore.and_then(to_value),
        ),
        (
            "models.routing".to_owned(),
            models.routing.and_then(to_value),
        ),
    ];
    out.push((
        "sandbox.enabled".to_owned(),
        env.sandbox.enabled.and_then(to_value),
    ));
    out.push((
        "sandbox.network".to_owned(),
        env.sandbox.network.as_ref().and_then(to_value),
    ));
    out.push((
        "sandbox.write".to_owned(),
        env.sandbox.write.as_ref().and_then(to_value),
    ));
    out.extend(
        models
            .providers
            .iter()
            .map(|(name, provider)| (format!("models.providers.{name}"), shown(provider))),
    );
    for (id, rule) in &env.rules {
        out.push((
            format!("rules.\"{id}\".classification"),
            rule.classification.as_ref().and_then(to_value),
        ));
        out.push((
            format!("rules.\"{id}\".agent"),
            rule.agent.as_ref().and_then(to_value),
        ));
    }
    out.into_iter()
        .filter_map(|(key, found)| Some((key, found?)))
        .collect()
}

fn to_value<T: Serialize>(value: T) -> Option<Value> {
    serde_json::to_value(value).ok()
}

/// A provider as it is shown. What may carry a secret is not: the values of
/// `env`, `headers`, `body` and the plugin `options` (their keys stay), and the
/// userinfo, query and fragment of `url`. The `command` is shown as it is.
fn shown(provider: &ProviderSpec) -> Option<Value> {
    let mut provider = provider.clone();
    provider.env.values_mut().for_each(hide);
    provider.headers.values_mut().for_each(hide);
    provider.options.values_mut().for_each(|v| *v = hidden());
    if let Some(body) = provider.body.as_mut() {
        body.values_mut().for_each(|v| *v = hidden());
    }
    provider.url = provider.url.as_deref().map(plain_url);
    to_value(provider)
}

fn hide(value: &mut String) {
    HIDDEN.clone_into(value);
}

fn hidden() -> Value {
    Value::from(HIDDEN)
}

/// `url` as `scheme://host[:port]/path`: without userinfo, query and fragment.
fn plain_url(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").map_or(("", url), |(s, r)| (s, r));
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let path = tail.split(['?', '#']).next().unwrap_or_default();
    if scheme.is_empty() {
        format!("{host}{path}")
    } else {
        format!("{scheme}://{host}{path}")
    }
}
