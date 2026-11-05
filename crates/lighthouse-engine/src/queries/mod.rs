//! The engine's queries on the graph: the one place that registers every input
//! and query kind with the [`Db`]. The files come first: `stat` and `content`
//! of a file, `dir` (a Merkle hash) and `files` (the file list); later layers
//! add theirs here.
//!
//! A walk sets the inputs, `stat` of every listed file and the entries each
//! directory lists; everything else is pulled. A file is read only when its
//! `stat` moved (or is racy, see [`stat_salt`]) and hashed once, as BLAKE3 for
//! the graph and as SHA-256 for the files the providers are told of, so that
//! neither is computed again while the file stays as it is.

mod content;
mod detect;

use std::path::Path;

use lighthouse_graph::{Db, Error, Input, Query};
use lighthouse_spec::Stat;
use serde::{Deserialize, Serialize};

pub(crate) use content::{Content, Readable};
pub(crate) use detect::{Detected, Graph};

/// The root of the project: what a path is relative to.
pub(crate) static ROOT: Input<(), String> = Input::new("root");
/// What decides the file list besides the file system: a hash of the
/// configuration's ignore patterns. `Stable`.
pub(crate) static CONFIG: Input<(), String> = Input::new("files-config");
/// The `lstat` of a listed file, with a salt that is not zero when the file
/// is too new to trust.
pub(crate) static STAT: Input<String, (Stat, u64)> = Input::new("stat");
/// What replaces a file for a check of an edit before it is written: an input
/// of its own so that an overlay never looks like a change on disk.
pub(crate) static OVERLAY: Input<String, Option<Content>> = Input::new("overlay");
/// What a directory lists, once ignored and vendored files are out: name and
/// whether it is a directory.
pub(crate) static ENTRIES: Input<String, Vec<(String, bool)>> = Input::new("entries");

pub(crate) static CONTENT: Query<String, Content> = Query::new("content", content::content);
pub(crate) static DIR: Query<String, Hash> = Query::new("dir", dir);
pub(crate) static FILES: Query<(), Vec<String>> = Query::new("files", files);

/// A directory with fewer entries than this is hashed on one thread.
const FAN_OUT: usize = 4;

pub(crate) type Hash = [u8; 32];

/// The state of the last walk, kept beside the graph.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Walked {
    pub started: Option<i64>,
    pub walks: u64,
    /// The salt of each file that was racy, by path.
    pub salts: std::collections::BTreeMap<String, u64>,
    pub dirs: lighthouse_spec::Dirs,
    pub overlaid: Vec<String>,
}

/// Makes every kind known to the graph.
pub(crate) fn register(db: &Db) {
    db.register_input(&ROOT);
    db.register_input(&CONFIG);
    db.register_input(&STAT);
    db.register_input(&OVERLAY);
    db.register_input(&ENTRIES);
    db.register_query(&CONTENT);
    db.register_query(&DIR);
    db.register_query(&FILES);
}

/// Whether a file is too new to trust the `stat` that was taken of it at the
/// last walk, which started at `since`: a change within the clock's tick of
/// the walk that hashed it does not show in its `stat`. The file is then
/// given a salt, a number of this walk, so that its input is a new value and
/// the file is read again; the salt stays while the file does, so that a
/// file read late enough to be sure is not read once more.
pub(crate) fn racy(mtime_ns: i64, since: Option<i64>) -> bool {
    since.is_some_and(|since| mtime_ns >= since.saturating_sub(lighthouse_spec::RACY_NS))
}

/// The Merkle hash of a directory: BLAKE3 over its listed children, sorted by
/// name, each as (name, kind, hash): the content of a file, the `dir` of a
/// directory.
fn dir(db: &Db, path: &String) -> Result<Hash, Error> {
    let entries = db.input(&ENTRIES, path)?;
    let at = |name: &str| Path::new(path).join(name).to_string_lossy().into_owned();
    let mut children: Vec<(String, String, bool)> = entries
        .iter()
        .map(|(name, is_dir)| (name.clone(), at(name), *is_dir))
        .collect();
    children.sort();
    let child = |db: &Db, (_, at, is_dir): &(String, String, bool)| -> Result<Hash, Error> {
        if *is_dir {
            Ok(*db.query(&DIR, at)?)
        } else {
            Ok(db.query(&CONTENT, at)?.blake3)
        }
    };
    let hashes: Vec<Result<Hash, Error>> = if children.len() < FAN_OUT {
        children.iter().map(|c| child(db, c)).collect()
    } else {
        let shared = std::sync::Arc::new(children.clone());
        let order: Vec<usize> = (0..shared.len()).collect();
        db.par_map(order, move |db, i| child(db, &shared[*i]))
    };
    let mut hasher = blake3::Hasher::new();
    for ((name, _, is_dir), hash) in children.iter().zip(hashes) {
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update(&[u8::from(*is_dir)]);
        hasher.update(&hash?);
    }
    Ok(hasher.finalize().into())
}

/// The project's files, sorted: every file the directories list, below the
/// root. It reads the ignore configuration so that a change of it is a new
/// list.
fn files(db: &Db, _: &()) -> Result<Vec<String>, Error> {
    db.input(&CONFIG, &())?;
    let (mut files, mut todo) = (Vec::new(), vec![String::new()]);
    while let Some(dir) = todo.pop() {
        for (name, is_dir) in db.input(&ENTRIES, &dir)?.iter() {
            let at = Path::new(&dir).join(name).to_string_lossy().into_owned();
            if *is_dir {
                todo.push(at);
            } else {
                files.push(at);
            }
        }
    }
    // In the order of paths, which compares by component, as the listing does.
    files.sort_by(|a, b| Path::new(a).cmp(Path::new(b)));
    Ok(files)
}
