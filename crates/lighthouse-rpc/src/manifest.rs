use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::Error;

pub const FILE_NAME: &str = "lighthouse-plugin.toml";

/// `lighthouse-plugin.toml`: how to start a plugin process.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub id: String,
    pub version: String,
    /// Executable. A relative path with a separator is resolved against the
    /// manifest directory; a bare name is looked up on `PATH`.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

impl PluginManifest {
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

pub fn load(dir: &Path) -> Result<Found, Error> {
    let path = dir.join(FILE_NAME);
    let text = fs::read_to_string(&path).map_err(|source| Error::Io {
        path: path.clone(),
        source,
    })?;
    let manifest = toml::from_str(&text).map_err(|source| Error::Manifest { path, source })?;
    Ok(Found {
        manifest,
        dir: dir.to_owned(),
    })
}

/// What a search found: usable manifests, and manifests that did not parse.
/// A broken manifest only matters to a plugin that is listed, so discovery
/// does not fail on it.
#[derive(Debug, Default)]
pub struct Discovered {
    pub found: Vec<Found>,
    pub broken: Vec<Error>,
}

/// Manifests in `<search>/*/lighthouse-plugin.toml` for every existing search
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
            .filter(|p| p.join(FILE_NAME).is_file())
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
