use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use lighthouse_model::{Format, Resource, STATE_DIR, Spec, documents, resource};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Error;

/// File name of a plugin manifest inside its directory.
pub const FILE_NAME: &str = "lighthouse-plugin.toml";

/// Names a plugin manifest may have, in the order they are tried; the format
/// follows the extension.
const FILE_NAMES: [&str; 3] = [
    FILE_NAME,
    "lighthouse-plugin.yaml",
    "lighthouse-plugin.json",
];

/// The spec of the `Plugin` kind: how to start a plugin process, and what the
/// plugin provides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginSpec {
    pub version: String,
    /// How to start the plugin's process; a bundled plugin, which the binary
    /// compiles in, has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<Runtime>,
    /// Directory of the plugin's decision catalog, relative to the manifest
    /// directory: the packs the plugin provides, read without starting the
    /// process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "provides_nothing")]
    pub provides: Provides,
}

impl Spec for PluginSpec {
    const KIND: &'static str = "Plugin";
}

/// How a plugin process is started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    /// Executable. A relative path with a separator is resolved against the
    /// manifest directory; a bare name is looked up on `PATH`.
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

/// What a plugin contributes, declared so that tools can list it without
/// starting the process. Each list is checked against what the process
/// reports when it starts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Provides {
    /// Language ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// Model ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
}

/// A `Plugin` document and the directory it was found in.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// `metadata.name` must equal the id the plugin reports.
    pub resource: Resource<PluginSpec>,
    pub dir: PathBuf,
}

impl Found {
    /// The id the plugin is known by.
    pub fn id(&self) -> &str {
        &self.resource.metadata.name
    }

    /// The directory of the plugin's decision catalog, when it ships one.
    pub fn catalog_dir(&self) -> Option<PathBuf> {
        self.resource
            .spec
            .catalog
            .as_ref()
            .map(|catalog| self.dir.join(catalog))
    }

    /// The executable, resolved against the manifest directory unless it is a
    /// bare name.
    /// `None` for a plugin with no process.
    pub(crate) fn command_path(&self) -> Option<PathBuf> {
        let command = &self.resource.spec.runtime.as_ref()?.command;
        Some(
            if Path::new(command).is_relative() && command.contains('/') {
                self.dir.join(command)
            } else {
                PathBuf::from(command)
            },
        )
    }

    /// The executable and its arguments, as they are started.
    pub(crate) fn argv(&self) -> Vec<String> {
        self.command_path()
            .map(|command| command.to_string_lossy().into_owned())
            .into_iter()
            .chain(self.args().iter().cloned())
            .collect()
    }

    /// The arguments the executable is started with.
    pub(crate) fn args(&self) -> &[String] {
        self.resource
            .spec
            .runtime
            .as_ref()
            .map_or(&[], |runtime| runtime.args.as_slice())
    }
}

/// What a search found: usable manifests, and manifests that did not parse.
/// A broken manifest only matters to a plugin that is listed, so discovery
/// does not fail on it.
#[derive(Debug, Default)]
pub struct Discovered {
    pub found: Vec<Found>,
    pub broken: Vec<Error>,
}

/// Where plugin manifests are searched: `<root>/.lighthouse/plugins`,
/// `$XDG_DATA_HOME/lighthouse/plugins` (default `~/.local/share/lighthouse/plugins`)
/// and `plugins` next to the executable. A
/// development checkout is reached only through an explicit `path`.
pub fn search_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.join(STATE_DIR).join("plugins")];
    dirs.extend(lighthouse_model::xdg_plugins());
    if let Some(dir) = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.join("plugins")))
    {
        dirs.push(dir);
    }
    dirs
}

/// Reads the manifest of `dir` (`lighthouse-plugin.toml`, `.yaml` or
/// `.json`); fails with `Io` when unreadable and `Manifest` when it is not a
/// valid `Plugin` document.
pub fn load(dir: &Path) -> Result<Found, Error> {
    let path = file_in(dir).unwrap_or_else(|| dir.join(FILE_NAME));
    let text = fs::read_to_string(&path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    let label = path.display().to_string();
    let manifest = |message: String| Error::Manifest {
        path: path.clone(),
        message,
    };
    let format = Format::of_path(&path).unwrap_or(Format::Toml);
    let mut docs = documents(format, &label, &text).map_err(|e| manifest(e.to_string()))?;
    if docs.len() != 1 {
        return Err(manifest(format!(
            "expected one Plugin document, found {}",
            docs.len()
        )));
    }
    let resource =
        resource::<PluginSpec>(&label, &docs.remove(0)).map_err(|e| manifest(e.to_string()))?;
    Ok(Found {
        resource,
        dir: dir.to_owned(),
    })
}

/// Manifests in `<search>/*/lighthouse-plugin.{toml,yaml,json}` for every existing search
/// directory, sorted by path. Directories without a manifest are ignored.
pub fn discover(search: &[PathBuf]) -> Result<Discovered, Error> {
    let mut out = Discovered::default();
    for dir in search {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(Error::Io {
                    path: dir.clone(),
                    source,
                });
            }
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| file_in(p).is_some())
            .collect();
        dirs.sort();
        for dir in dirs {
            match load(&dir) {
                Ok(found) => out.found.push(found),
                Err(e) => out.broken.push(e),
            }
        }
    }
    Ok(out)
}

/// The manifest file in `dir`, if it has one.
fn file_in(dir: &Path) -> Option<PathBuf> {
    lighthouse_model::file_in(dir, &FILE_NAMES)
}

fn provides_nothing(provides: &Provides) -> bool {
    provides == &Provides::default()
}
