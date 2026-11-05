//! The units of a language and what a provider gave for each: `units`, the
//! files grouped as the language's manifest says; `unit`, the key of one;
//! `fragment`, the last answer of the provider, set by the engine; and `api`,
//! what an importer sees of it.
//!
//! A provider is an action outside the graph, as in Bazel and Buck2: the graph
//! says which units are stale (their key moved, or the `api` of a unit they
//! imported did) and the answer comes back as an input.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lighthouse_graph::{Durability, Error, Graph, Input, Query};
use lighthouse_model::ucm::{Fragment, Host, key};
use lighthouse_plugin::UnitsBy;
use lighthouse_spec::{CONTENT, FILES, Hash};
use serde::{Deserialize, Serialize};

/// An import that stands for every unit of the language: a provider whose
/// analysis of a unit reads what the others declare (a name looked up in the
/// whole project, a module tree of all the crates) imports it, and the unit is
/// stale when the `api` of any other unit moved.
pub(crate) const EVERY: &str = "*";

/// Two seconds, the coarsest granularity of a file system's timestamps.
const RACY_NS: i64 = 2_000_000_000;

/// The language of a provider, by id: its [`Provider`].
pub(crate) static PROVIDER: Input<String, Provider> = Input::new("provider");
/// The files of the project a language claims and the engine could read,
/// sorted: set from the routing, which is the host's configuration.
pub(crate) static CLAIMED: Input<String, Vec<String>> = Input::new("claimed");
/// What a provider said of the unit `(language, id)` besides its fragments:
/// the key it was asked under, its `api` and imports, what it noticed. Small,
/// so the host reads it for every unit of every run. Persisted: a run that
/// finds the unit's key as it was takes it and starts no provider.
pub(crate) static FRAGMENT: Input<(String, String), Stored> = Input::new("fragment").persisted();
pub(crate) static UNITS: Query<String, Vec<Unit>> = Query::new("units", units);
pub(crate) static SETTINGS: Query<String, Hash> = Query::new("settings", settings);
pub(crate) static UNIT: Query<(String, String), Hash> = Query::new("unit", unit);
pub(crate) static API: Query<(String, String), String> = Query::new("api", api);

/// What decides the answers of a language's provider besides the files, and
/// how its files make units. `Static`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Provider {
    /// The provider's id and version, the digest of its binary, its options and
    /// the other things the key of a unit covers.
    pub identity: String,
    pub by: By,
    pub manifest: String,
    pub settings: Vec<String>,
}

/// How a language's files make units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum By {
    Directory,
    File,
    Manifest,
}

impl From<UnitsBy> for By {
    fn from(by: UnitsBy) -> Self {
        match by {
            UnitsBy::Directory => Self::Directory,
            UnitsBy::File => Self::File,
            UnitsBy::Manifest => Self::Manifest,
        }
    }
}

/// A unit: its id and its files, project-relative and sorted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Unit {
    pub id: String,
    pub files: Vec<String>,
}

/// What the provider gave for a unit, and what it was given.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Stored {
    /// The key of the unit it was asked for.
    pub key: String,
    /// The answer has no notice or gap: a later run may take it as it is.
    pub clean: bool,
    /// The units it imports, each with the `api` they had when it was stored.
    pub imports: Vec<(String, String)>,
    pub body: String,
    pub api: String,
    /// The files outside the project the answer read, as they were when it
    /// was stored.
    pub reads: Vec<Read>,
    /// When it was stored, in nanoseconds since the epoch: a file modified
    /// near that moment cannot be told apart by its stat.
    pub at: i64,
    /// Messages about the unit that leave its analysis complete.
    pub notices: Vec<String>,
    /// The files that could not be analyzed, by path and reason.
    pub gaps: Vec<(Option<std::path::PathBuf>, String)>,
    /// The answer has source trees of host files.
    pub sources: bool,
}

/// A file outside the project that an answer depended on, with what the host
/// saw of it: the discovered dependency of a build action (a depfile).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Read {
    pub path: String,
    /// Length and modification time; none when the file was not there.
    pub stat: Option<(u64, i64)>,
    pub hash: String,
}

impl Read {
    /// What the host sees of the file at `path` now.
    pub fn of(path: &str) -> Self {
        let stat = stat_of(path);
        let hash = stat.map_or_else(String::new, |_| hash_of(path));
        Self {
            path: path.to_owned(),
            stat,
            hash,
        }
    }

    /// Whether the file is not what it was: its stat moved and its content
    /// differs. A stat that did not move is trusted unless the file was
    /// modified within [`RACY_NS`] of `at`, when it is read again.
    pub fn moved(&self, at: i64) -> bool {
        let now = stat_of(&self.path);
        if now.is_some() && now == self.stat && now.is_some_and(|(_, m)| m + RACY_NS < at) {
            return false;
        }
        match now {
            None => self.stat.is_some(),
            Some(_) => hash_of(&self.path) != self.hash,
        }
    }
}

/// Sets the inputs of a project that is the given fragments, one unit per
/// file, with nothing else known of the files: what the keys of fragments made
/// without a run are read from.
pub(crate) fn set_units(graph: &mut Graph, fragments: &[Fragment]) -> Result<(), Error> {
    for fragment in fragments {
        let Some(file) = fragment.files.first() else {
            continue;
        };
        let (language, path) = (file.lang.clone(), key(&file.path));
        let provider = Provider {
            identity: language.clone(),
            by: By::File,
            manifest: String::new(),
            settings: Vec::new(),
        };
        graph.set_input(&PROVIDER, &language, provider, Durability::Static)?;
        let stored = Stored {
            key: path.clone(),
            clean: true,
            ..Stored::default()
        };
        graph.set_input(&FRAGMENT, &(language, path), stored, Durability::Volatile)?;
    }
    Host::set_units(graph, fragments)
}

/// The units of a language, sorted by id: it reads the paths of the files and
/// nothing else, so a body edit never moves it.
fn units(graph: &Graph, language: &String) -> Result<Vec<Unit>, Error> {
    let Some(provider) = graph.input(&PROVIDER, language)? else {
        return Ok(Vec::new());
    };
    let claimed = graph.input(&CLAIMED, language)?.unwrap_or_default();
    let roots = match provider.by {
        By::Manifest => manifests(graph, &provider.manifest)?,
        By::Directory | By::File => BTreeSet::new(),
    };
    let mut by_id: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in claimed.iter() {
        let id = match provider.by {
            By::File => path.clone(),
            By::Directory => parent(path),
            By::Manifest => root_of(&roots, path),
        };
        by_id.entry(id).or_default().push(path.clone());
    }
    Ok(by_id
        .into_iter()
        .map(|(id, files)| Unit { id, files })
        .collect())
}

/// The root directory is `.`.
fn parent(path: &str) -> String {
    match Path::new(path).parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_string_lossy().into_owned(),
        _ => ".".to_owned(),
    }
}

/// The directories that hold a file named `manifest`.
fn manifests(graph: &Graph, manifest: &str) -> Result<BTreeSet<String>, Error> {
    let files = graph.query(&FILES, &())?;
    Ok(files
        .iter()
        .filter(|path| {
            Path::new(path)
                .file_name()
                .is_some_and(|name| name == manifest)
        })
        .map(|path| parent(path))
        .collect())
}

/// The nearest directory above `path` that holds a manifest; a file below
/// none is a unit of its own.
fn root_of(roots: &BTreeSet<String>, path: &str) -> String {
    let mut dir = parent(path);
    loop {
        if roots.contains(&dir) {
            return dir;
        }
        if dir == "." {
            return path.to_owned();
        }
        dir = parent(&dir);
    }
}

/// The content of every settings file of the language, by path.
fn settings(graph: &Graph, language: &String) -> Result<Hash, Error> {
    let mut hasher = blake3::Hasher::new();
    let Some(provider) = graph.input(&PROVIDER, language)? else {
        return Ok(hasher.finalize().into());
    };
    let files = graph.query(&FILES, &())?;
    for path in files.iter() {
        let named = Path::new(path).file_name().is_some_and(|name| {
            provider
                .settings
                .iter()
                .any(|setting| name == setting.as_str())
        });
        if named {
            hasher.update(path.as_bytes());
            hasher.update(&graph.query(&CONTENT, path)?.blake3);
        }
    }
    Ok(hasher.finalize().into())
}

/// The key of a unit: the provider's identity, the settings files, and its
/// member paths with their content.
fn unit(graph: &Graph, (language, id): &(String, String)) -> Result<Hash, Error> {
    let mut hasher = blake3::Hasher::new();
    let provider = graph.input(&PROVIDER, language)?;
    if let Some(provider) = &provider {
        hasher.update(provider.identity.as_bytes());
    }
    hasher.update(&*graph.query(&SETTINGS, language)?);
    hasher.update(id.as_bytes());
    let units = graph.query(&UNITS, language)?;
    let found = units.binary_search_by(|unit| unit.id.as_str().cmp(id.as_str()));
    for path in found.map_or(&[][..], |at| units[at].files.as_slice()) {
        hasher.update(&(path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(&graph.query(&CONTENT, path)?.blake3);
    }
    Ok(hasher.finalize().into())
}

/// What importers see of the unit: the `api` of its last answer, empty when it
/// has none. It reads `fragment` alone, so a unit that imports another reads
/// nothing of its body. Of every unit, it is the `api` of all of them.
fn api(graph: &Graph, unit: &(String, String)) -> Result<String, Error> {
    if unit.1 == EVERY {
        return every(graph, &unit.0);
    }
    Ok(graph
        .input(&FRAGMENT, unit)?
        .map_or_else(String::new, |stored| stored.api.clone()))
}

/// The `api` of every unit of the language, together.
fn every(graph: &Graph, language: &String) -> Result<String, Error> {
    let mut hasher = blake3::Hasher::new();
    for unit in graph.query(&UNITS, language)?.iter() {
        let api = graph.query(&API, &(language.clone(), unit.id.clone()))?;
        for part in [unit.id.as_str(), api.as_str()] {
            hasher.update(&(part.len() as u64).to_le_bytes());
            hasher.update(part.as_bytes());
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn stat_of(path: &str) -> Option<(u64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let nanos = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((meta.len(), i64::try_from(nanos).ok()?))
}

fn hash_of(path: &str) -> String {
    std::fs::read(path).map_or_else(
        |_| String::new(),
        |data| blake3::hash(&data).to_hex().to_string(),
    )
}
