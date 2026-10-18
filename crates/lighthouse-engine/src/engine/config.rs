//! Reading a configuration against the registry: which rules are active,
//! whether the plugins and rules it names exist, and what its languages say.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

use lighthouse_model::ModelRef;
use lighthouse_plugin::Registry;
use lighthouse_spec::{Config, ModelProvider, Models, Projects, Task, glob_set};

use super::{Error, Language};

/// The rules the configuration enables for at least one file: the projects it
/// extends and the entries it sets, resolved over `projects`.
pub fn active(config: &Config, projects: &Projects) -> Result<BTreeSet<String>, Error> {
    let mut active: BTreeSet<String> = config
        .resolve(Path::new(""), "", projects)?
        .iter()
        .filter_map(|(id, c)| c.level.map(|_| id.clone()))
        .collect();
    let overridden = config.configured().filter(|(_, c)| c.level.is_some());
    active.extend(overridden.map(|(id, _)| id.to_owned()));
    Ok(active)
}

/// Rejects a configuration that names plugins, projects or rules the registry
/// lacks, lists them inconsistently, or gives a rule invalid options.
pub(super) fn validate_config(
    registry: &Registry,
    config: &Config,
    binding: &Models,
    projects: &Projects,
) -> Result<(), Error> {
    for plugin in config.plugins() {
        if !registry.has_plugin(&plugin.id) {
            return Err(Error::UnknownPlugin(plugin.id.clone()));
        }
    }
    registry.validate()?;
    validate_models(registry, config, binding)?;
    let listed = |id: &str| config.lists(lighthouse_plugin::plugin_of(id));
    let entries = config.entries(projects)?;
    for (id, config) in entries {
        let rule = registry
            .rule(id)
            .ok_or_else(|| Error::UnknownRule(id.to_owned()))?;
        if !listed(id) {
            return Err(Error::PluginNotListed(id.to_owned()));
        }
        rule.validate(&config.options)?;
    }
    Ok(())
}

/// The constructor prefixes of each language: the project's, else the ones its
/// provider declares.
pub(super) fn constructors(registry: &Registry, config: &Config) -> BTreeMap<String, Vec<String>> {
    let mut found: BTreeMap<String, Vec<String>> = registry
        .languages()
        .iter()
        .map(|language| {
            let manifest = language.provider.manifest();
            (
                manifest.id.clone(),
                manifest.conventions.constructor_prefixes.clone(),
            )
        })
        .collect();
    for (id, prefixes) in config.constructor_prefixes() {
        found.insert(id.to_owned(), prefixes.to_vec());
    }
    found
}

/// The language providers of the registry, and which of them listed plugins enable.
pub(super) fn load_languages(
    registry: &Registry,
    config: &Config,
) -> Result<(Vec<usize>, Vec<Language>), Error> {
    let mut providers = Vec::new();
    let mut languages = Vec::new();
    for entry in registry.languages() {
        if config.lists(&entry.plugin) {
            providers.push(languages.len());
        }
        languages.push(Language {
            files: glob_set(&entry.provider.manifest().globs)?,
            tests: glob_set(&entry.provider.manifest().conventions.test_globs)?,
        });
    }
    Ok((providers, languages))
}

pub(super) fn io_error(path: &Path) -> impl FnOnce(io::Error) -> Error {
    |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

/// Rejects a binding to a model whose provider is unknown or does not serve the
/// task bound, and a provider that cannot work.
fn validate_models(registry: &Registry, config: &Config, binding: &Models) -> Result<(), Error> {
    if let Some((name, reason)) = binding.invalid.iter().next() {
        return Err(Error::Model {
            name: name.clone(),
            reason: reason.clone(),
        });
    }
    for (_, reference) in binding.bindings() {
        let known = ModelRef::parse(reference)
            .is_some_and(|model| binding.provider(model.provider).is_some());
        if !known {
            return Err(Error::UnknownModel(reference.to_owned()));
        }
    }
    let used = binding
        .bindings()
        .filter_map(|(_, reference)| ModelRef::parse(reference))
        .map(|model| model.provider);
    let providers: BTreeMap<&str, ModelProvider> = used
        .filter_map(|name| Some((name, binding.provider(name)?)))
        .chain(
            binding
                .providers
                .iter()
                .map(|(n, p)| (n.as_str(), p.clone())),
        )
        .collect();
    for (name, provider) in providers {
        let reason = match &provider {
            _ if binding.providers.contains_key(name)
                && lighthouse_spec::RESERVED_PROVIDERS.contains(&name) =>
            {
                Some("the name is reserved".to_owned())
            }
            ModelProvider::Rpc { plugin, .. } if !config.lists(plugin) => {
                Some(format!("plugin `{plugin}` is not listed in `plugins`"))
            }
            ModelProvider::Rpc { plugin, .. } if registry.model(plugin).is_none() => {
                Some(format!("plugin `{plugin}` offers no model"))
            }
            _ => None,
        };
        if let Some(reason) = reason {
            return Err(Error::Model {
                name: name.to_owned(),
                reason,
            });
        }
    }
    for (task, reference) in binding.bindings() {
        let provider = ModelRef::parse(reference)
            .and_then(|model| Some((model.provider, binding.provider(model.provider)?)));
        if let Some((name, provider)) = provider
            && !serves(registry, &provider, task)
        {
            return Err(Error::Model {
                name: reference.to_owned(),
                reason: format!(
                    "provider `{name}` does not serve `{task}`{}",
                    only_for(task)
                ),
            });
        }
    }
    Ok(())
}

/// Whether `provider` serves `task`: what its protocol says, else what its
/// plugin said in the handshake.
fn serves(registry: &Registry, provider: &ModelProvider, task: Task) -> bool {
    let protocol = provider.serves(task);
    let ModelProvider::Rpc { plugin, .. } = provider else {
        return protocol.unwrap_or(true);
    };
    let listed = registry.model(plugin).is_some_and(|model| {
        let capabilities = &model.manifest().capabilities;
        let capability = match task {
            Task::Classification => capabilities.classification,
            Task::Edit => None,
        };
        capability.is_some_and(|capability| capability.supported)
    });
    protocol.unwrap_or(true) && listed
}

/// What a task needs of the provider that serves it, when it is not what the
/// provider's kind alone says.
fn only_for(task: Task) -> &'static str {
    match task {
        Task::Edit => ": only a command without a protocol does",
        Task::Classification => "",
    }
}
