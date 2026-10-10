//! Reading a configuration against the registry: which rules are active,
//! whether the plugins and rules it names exist, and what its languages say.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

use lighthouse_plugin::Registry;
use lighthouse_spec::{Config, Projects, glob_set};

use super::{Error, Language};

/// The rules the configuration enables for at least one file: the projects it
/// extends and the entries it sets, resolved over `projects`.
pub fn active_rules(config: &Config, projects: &Projects) -> Result<BTreeSet<String>, Error> {
    let mut active: BTreeSet<String> = config
        .resolve(Path::new(""), "", projects)?
        .into_iter()
        .filter_map(|(id, c)| c.level.map(|_| id))
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
    projects: &Projects,
) -> Result<(), Error> {
    for plugin in config.plugins() {
        if !registry.has_plugin(&plugin.id) {
            return Err(Error::UnknownPlugin(plugin.id.clone()));
        }
    }
    registry.validate()?;
    let listed = |id: &str| config.lists(lighthouse_plugin::plugin_of(id));
    let entries = config.layer().entries(projects)?;
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
        .map(|(_, provider)| {
            let manifest = provider.manifest();
            (
                manifest.id.clone(),
                manifest.conventions.constructor_prefixes.clone(),
            )
        })
        .collect();
    for (id, prefixes) in config.constructor_prefixes() {
        found.insert(id.clone(), prefixes.clone());
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
    for (plugin, provider) in registry.languages() {
        if config.lists(plugin) {
            providers.push(languages.len());
        }
        languages.push(Language {
            files: glob_set(&provider.manifest().globs)?,
            tests: glob_set(&provider.manifest().conventions.test_globs)?,
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
