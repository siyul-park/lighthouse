use std::{
    fs, io,
    path::{Path, PathBuf},
};

use lighthouse_resource::{Format, Spec, documents, resource};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Error;

/// File name of a plugin manifest inside its directory.
pub const FILE_NAME: &str = "lighthouse-plugin.toml";

/// Names a plugin manifest may have, in the order they are tried; the format
/// follows the extension.
pub const FILE_NAMES: [&str; 3] = [
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
    pub runtime: Runtime,
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
/// starting the process. `languages` is checked against what the process
/// reports when it starts; the other lists are read by the tooling that
/// handles their kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provides {
    /// Language ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// Catalog directories of decisions, relative to the manifest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub embedders: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fix_ops: Vec<String>,
}

/// A plugin manifest: the `Plugin` document of a plugin directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    /// `metadata.name`; must equal the id the plugin reports.
    pub id: String,
    pub version: String,
    /// Executable. A relative path with a separator is resolved against the
    /// manifest directory; a bare name is looked up on `PATH`.
    pub command: String,
    pub args: Vec<String>,
    pub provides: Provides,
}

impl PluginManifest {
    /// Reads a parsed `Plugin` document.
    pub fn from_resource(plugin: lighthouse_resource::Resource<PluginSpec>) -> Self {
        let lighthouse_resource::Resource { metadata, spec } = plugin;
        Self {
            id: metadata.name,
            version: spec.version,
            command: spec.runtime.command,
            args: spec.runtime.args,
            provides: spec.provides,
        }
    }

    pub(crate) fn command_path(&self, dir: &Path) -> PathBuf {
        let command = Path::new(&self.command);
        if command.is_relative() && self.command.contains('/') {
            dir.join(command)
        } else {
            command.to_owned()
        }
    }
}

/// A manifest and the directory it was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub manifest: PluginManifest,
    pub dir: PathBuf,
}

/// What a search found: usable manifests, and manifests that did not parse.
/// A broken manifest only matters to a plugin that is listed, so discovery
/// does not fail on it.
#[derive(Debug, Default)]
pub struct Discovered {
    pub found: Vec<Found>,
    pub broken: Vec<Error>,
}

/// The manifest file in `dir`, if it has one.
pub fn file_in(dir: &Path) -> Option<PathBuf> {
    lighthouse_resource::file_in(dir, &FILE_NAMES)
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
    let manifest = parse(
        Format::of_path(&path).unwrap_or(Format::Toml),
        &label,
        &text,
    )
    .map_err(|message| Error::Manifest {
        path: path.clone(),
        message,
    })?;
    Ok(Found {
        manifest,
        dir: dir.to_owned(),
    })
}

/// Parses the text of a manifest.
pub fn parse(format: Format, path: &str, text: &str) -> Result<PluginManifest, String> {
    let mut docs = documents(format, path, text).map_err(|e| e.to_string())?;
    if docs.len() != 1 {
        return Err(format!(
            "expected one Plugin document, found {}",
            docs.len()
        ));
    }
    resource::<PluginSpec>(path, &docs.remove(0))
        .map(PluginManifest::from_resource)
        .map_err(|e| e.to_string())
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

fn provides_nothing(provides: &Provides) -> bool {
    provides == &Provides::default()
}
