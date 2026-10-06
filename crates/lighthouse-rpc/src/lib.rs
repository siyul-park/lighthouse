mod client;
mod convert;
mod manifest;
mod plugin;

use std::{
    env,
    path::{Path, PathBuf},
    time::Duration,
};

use lighthouse_config::{Config, PluginRef};
use lighthouse_model::Incomplete;
use lighthouse_plugin::{Plugin, Registry};
use thiserror::Error;

pub use manifest::{Discovered, FILE_NAME, Found, PluginManifest, discover, load};
pub use plugin::RpcPlugin;

/// Per-request limit unless the plugin entry sets `timeout`; generous because
/// a first index may build a cold cache.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// Why discovering, starting or registering an external plugin failed. Every
/// variant but `Unavailable` is a configuration error that aborts the run.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{}: {source}", path.display())]
    Manifest {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("plugin `{id}` is provided more than once: {}", places.join(", "))]
    Conflict { id: String, places: Vec<String> },
    #[error("{}: manifest declares `{found}`, config lists `{id}`", dir.display())]
    IdMismatch {
        id: String,
        found: String,
        dir: PathBuf,
    },
    #[error("plugin `{id}`: {reason}")]
    Plugin { id: String, reason: String },
    /// The process started but its handshake did not complete. Not a
    /// configuration error: the run goes on and reports the gap.
    #[error("plugin `{id}` is unavailable: {reason}")]
    Unavailable {
        id: String,
        reason: String,
        notices: Vec<String>,
    },
}

/// What registering external plugins left for the run to report.
#[derive(Debug, Default)]
pub struct Registered {
    /// Ignored broken manifests and what unavailable plugins wrote to stderr.
    pub notices: Vec<String>,
    /// One entry per listed plugin that started but failed its handshake; pass
    /// to the engine so the run is reported incomplete.
    pub incomplete: Vec<Incomplete>,
}

/// A listed plugin whose handshake failed: registered without languages so the
/// configuration stays valid, while its gap is reported as incomplete.
struct Unavailable(lighthouse_plugin::Manifest);

impl Plugin for Unavailable {
    fn manifest(&self) -> lighthouse_plugin::Manifest {
        self.0.clone()
    }
}

/// Where plugin manifests are searched: `<root>/.lighthouse/plugins`,
/// `~/.lighthouse/plugins` and `plugins` next to the executable. A
/// development checkout is reached only through an explicit `path`.
pub fn search_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.join(".lighthouse/plugins")];
    if let Some(home) = env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".lighthouse/plugins"));
    }
    if let Some(dir) = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.join("plugins")))
    {
        dirs.push(dir);
    }
    dirs
}

/// Starts every plugin listed in `config` that is not already registered and
/// registers it. An entry with `path` uses that directory (relative to
/// `root`); other ids are looked up in `search`. An id provided by more than
/// one place, a registered in-process plugin included, is an error, as are a
/// protocol version or identity mismatch and a missing binary. A process that
/// starts but crashes, times out or answers badly during `initialize` is
/// reported as incomplete instead. Ids found nowhere are left for the engine
/// to report as unknown. Manifests of unlisted plugins are never started and,
/// when broken, ignored.
pub fn register(
    registry: &mut Registry,
    config: &Config,
    root: &Path,
    search: &[PathBuf],
) -> Result<Registered, Error> {
    let discovered = discover(search)?;
    let mut registered = Registered::default();
    note_broken(discovered.broken, config, &mut registered)?;
    for listed in config.plugins() {
        let id = listed.id.as_str();
        let chosen = choose(listed, registry.has_plugin(id), &discovered.found, root)?;
        let Some(one) = chosen.first() else { continue };
        let timeout = listed.timeout().unwrap_or(DEFAULT_TIMEOUT);
        let registration = match RpcPlugin::connect(one, root, timeout) {
            Ok(plugin) => registry.register(&plugin),
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
                registry.register(&Unavailable(lighthouse_plugin::Manifest {
                    id: one.manifest.id.clone(),
                    version: one.manifest.version.clone(),
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
            if one.manifest.id != id {
                return Err(Error::IdMismatch {
                    id: id.to_owned(),
                    found: one.manifest.id,
                    dir,
                });
            }
            vec![one]
        }
        None => found
            .iter()
            .filter(|f| f.manifest.id == id)
            .cloned()
            .collect(),
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
