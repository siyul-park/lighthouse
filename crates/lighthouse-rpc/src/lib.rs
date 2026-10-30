//! The adapter that makes a plugin process a `Plugin`: it starts the process
//! and converts the wire types to the core ones. It does not depend on the
//! engine.

mod agent;
mod client;
mod command;
mod convert;
mod error;
mod events;
mod manifest;
mod model;
mod plugin;
mod shell;

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use lighthouse_model::{Incomplete, STATE_DIR};
use lighthouse_plugin::{Plugin, Registry};
use lighthouse_spec::{Config, PluginRef};
use thiserror::Error;

pub use command::CommandModel;
pub use error::Error;
pub use manifest::{FILE_NAME, Found, PluginSpec, Provides, discover, load, search_dirs};
pub use plugin::RpcPlugin;

/// Per-request limit unless the plugin entry sets `timeout`; generous because
/// a first index may build a cold cache.
const PLUGIN_TIMEOUT: Duration = Duration::from_secs(600);

/// What registering external plugins left for the run to report.
#[derive(Debug, Default)]
pub struct Registered {
    /// Ignored broken manifests and what unavailable plugins wrote to stderr.
    pub notices: Vec<String>,
    /// One entry per listed plugin that started but failed its handshake; pass
    /// to the engine so the run is reported incomplete.
    pub incomplete: Vec<Incomplete>,
}

/// A listed plugin that is part of the project, and the program it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPlugin {
    pub id: String,
    /// The executable and its arguments, as they are started.
    pub argv: Vec<String>,
}

/// The decision catalog of a listed plugin, not yet read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCatalog {
    pub plugin: String,
    pub dir: PathBuf,
    /// Whether the plugin comes with the project, so that its catalog is
    /// trusted with the project (see [`local_plugins`]).
    pub local: bool,
}

/// A listed plugin whose handshake failed: registered without languages so the
/// configuration stays valid, while its gap is reported as incomplete.
struct Unavailable(lighthouse_plugin::PluginManifest);

impl Plugin for Unavailable {
    fn manifest(&self) -> &lighthouse_plugin::PluginManifest {
        &self.0
    }
}

/// The JSON Schema of the kinds this crate defines.
pub fn descriptors() -> Vec<lighthouse_model::Schema> {
    vec![lighthouse_model::Schema::new::<PluginSpec>()]
}

/// Starts every plugin listed in `config` that is not already registered and
/// registers it. An entry with `path` uses that directory (relative to
/// `root`); other ids are looked up in `search`. An id provided by more than
/// one place, a registered in-process plugin included, is an error, as are a
/// protocol version or identity mismatch and a missing binary. A process that
/// starts but crashes, times out or answers badly during `initialize` is
/// reported as incomplete instead. Ids found nowhere are left for the engine
/// to report as unknown. Manifests of unlisted plugins are never started and,
/// when broken, ignored. A plugin that provides models is started in `store`,
/// the root of the datasets and models, which is `root` unless a run spans
/// several projects; any other plugin is started in `root`.
pub fn register(
    registry: &mut Registry,
    config: &Config,
    root: &Path,
    store: &Path,
    search: &[PathBuf],
) -> Result<Registered, Error> {
    let discovered = discover(search)?;
    let mut registered = Registered::default();
    note_broken(discovered.broken, config, &mut registered)?;
    for listed in config.plugins() {
        let id = listed.id.as_str();
        let chosen = choose(listed, registry.has_plugin(id), &discovered.found, root)?;
        let Some(one) = chosen.first() else { continue };
        let timeout = listed.timeout().unwrap_or(PLUGIN_TIMEOUT);
        let at = if one.resource.spec.provides.models.is_empty() {
            root
        } else {
            store
        };
        let registration = match RpcPlugin::connect(one, at, timeout) {
            Ok(plugin) => registry.register(&plugin.confined(config.sandbox(), root)),
            Err(Error::Unavailable {
                id,
                reason,
                notices,
            }) => {
                registered.notices.extend(notices);
                registered.incomplete.push(Incomplete {
                    path: None,
                    reason: format!(
                        "plugin `{id}` is unavailable, its files fall back to plain text: {reason}"
                    ),
                });
                registry.register(&Unavailable(lighthouse_plugin::PluginManifest {
                    id: one.id().to_owned(),
                    version: one.resource.spec.version.clone(),
                }))
            }
            Err(e) => return Err(e),
        };
        registration.map_err(|e| Error::Plugin {
            id: id.to_owned(),
            reason: e.to_string(),
        })?;
    }
    Ok(registered)
}

/// The listed plugins the project brings: those in its `.lighthouse/plugins`
/// and those its file names by a `path`, wherever it points. They are code the
/// repository chose, which runs only in a project the user trusts. Those found
/// in the user's plugin directory or next to the executable are not among
/// them.
pub fn local_plugins(config: &Config, root: &Path, search: &[PathBuf]) -> Vec<LocalPlugin> {
    let found = discover(search).map(|d| d.found).unwrap_or_default();
    let own = canonical(&root.join(STATE_DIR).join("plugins"));
    let root = canonical(root);
    config
        .plugins()
        .iter()
        .filter_map(|listed| {
            let one = choose(listed, false, &found, &root)
                .ok()?
                .into_iter()
                .next()?;
            comes_with_project(listed, &one, &own).then(|| LocalPlugin {
                id: one.id().to_owned(),
                argv: one.argv(),
            })
        })
        .collect()
}

/// The catalogs of the listed plugins that ship one, in listing order, found
/// the way [`register`] finds the plugins. A listed plugin that is found
/// nowhere ships none here: the engine reports it as unknown. A broken
/// manifest of a listed plugin is an error.
pub fn plugin_catalogs(
    config: &Config,
    root: &Path,
    search: &[PathBuf],
) -> Result<Vec<PluginCatalog>, Error> {
    let discovered = discover(search)?;
    note_broken(discovered.broken, config, &mut Registered::default())?;
    let own = canonical(&root.join(STATE_DIR).join("plugins"));
    let root = canonical(root);
    let mut catalogs = Vec::new();
    for listed in config.plugins() {
        let Some(one) = choose(listed, false, &discovered.found, &root)?
            .into_iter()
            .next()
        else {
            continue;
        };
        let Some(dir) = one.catalog_dir() else {
            continue;
        };
        catalogs.push(PluginCatalog {
            plugin: one.id().to_owned(),
            dir,
            local: comes_with_project(listed, &one, &own),
        });
    }
    Ok(catalogs)
}

/// The plugin that provides the pack `pack`, for the message of a config that
/// uses a pack whose plugin is not loaded: the one of `listed` or of the
/// search locations whose catalog holds the pack, else the plugin of the
/// language, which is named `lang-<pack>`.
pub fn plugin_of_pack(pack: &str, listed: &[PluginCatalog], search: &[PathBuf]) -> String {
    let holds = |dir: &Path| dir.join(pack).join("pack.yaml").is_file();
    if let Some(one) = listed.iter().find(|one| holds(&one.dir)) {
        return one.plugin.clone();
    }
    let found = discover(search).map(|d| d.found).unwrap_or_default();
    found
        .iter()
        .find(|one| one.catalog_dir().is_some_and(|dir| holds(&dir)))
        .map_or_else(|| format!("lang-{pack}"), |one| one.id().to_owned())
}

/// Whether the listed plugin `one` is part of the project: it has a `path`, or
/// sits in the project's own plugin directory `own`. The directory a plugin
/// sits in is resolved, the plugin's own directory is not: one that links
/// elsewhere is still the project's.
fn comes_with_project(listed: &PluginRef, one: &Found, own: &Path) -> bool {
    listed.path.is_some() || one.dir.parent().is_some_and(|dir| canonical(dir) == own)
}

/// `path` with its links resolved, else as it is.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

/// A broken manifest of a listed plugin is an error; any other is a notice.
fn note_broken(
    broken: Vec<Error>,
    config: &Config,
    registered: &mut Registered,
) -> Result<(), Error> {
    for broken in broken {
        let listed = match &broken {
            Error::Manifest { path, .. } => path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| config.lists(&name.to_string_lossy())),
            _ => false,
        };
        if listed {
            return Err(broken);
        }
        registered
            .notices
            .push(format!("ignored plugin manifest: {broken}"));
    }
    Ok(())
}

/// The manifests that provide the listed plugin: its `path` entry, else every
/// discovered match. More than one place, built in included, is a conflict.
fn choose(
    listed: &PluginRef,
    builtin: bool,
    found: &[Found],
    root: &Path,
) -> Result<Vec<Found>, Error> {
    let id = listed.id.as_str();
    let chosen = match &listed.path {
        Some(path) => {
            let dir = root.join(path);
            let one = load(&dir)?;
            if one.id() != id {
                return Err(Error::IdMismatch {
                    id: id.to_owned(),
                    found: one.id().to_owned(),
                    dir,
                });
            }
            vec![one]
        }
        None => found.iter().filter(|f| f.id() == id).cloned().collect(),
    };
    if chosen.len() + usize::from(builtin) > 1 {
        let mut places: Vec<String> = chosen.iter().map(|f| f.dir.display().to_string()).collect();
        if builtin {
            places.insert(0, "built in".to_owned());
        }
        return Err(Error::Conflict {
            id: id.to_owned(),
            places,
        });
    }
    Ok(chosen)
}
