//! The file kinds of the graph: the inputs `root`, `config`, `stat` and
//! `overlay`, and `entries`, `content`, `tree` (a Merkle hash) and `files`
//! (the file list) pulled from them. Later layers add theirs in the crates
//! that own them and read these through the statics below.
//!
//! A walk sets `stat` of every directory it enters and every file it lists.
//! That an input is set is what puts a file in the project: an ignored or
//! vendored file has none, so `tree` and `files` pass over it. Everything else
//! is pulled. A directory is read when its `stat` moved, a file when its
//! `stat` moved (or is racy, see [`Stamped`]) and hashed once, as BLAKE3 for
//! the graph and as SHA-256 for the files the providers are told of, so that
//! neither is computed again while the file stays as it is.

mod content;
mod held;

use std::{path::Path, sync::Arc};

use lighthouse_graph::{Error, Graph, Input, Query};
pub use lighthouse_model::ucm::key;

pub use content::{Content, Readable};
pub use held::Held;

use crate::{Entries, Kind, Stat, read_entries};

/// A directory with fewer entries than this is hashed on one thread.
const FAN_OUT: usize = 4;

/// The root of the project: what a path is relative to.
pub static ROOT: Input<(), String> = Input::new("root");
/// What decides the file list besides the file system: a hash of the
/// configuration's ignore patterns. `Stable`.
pub static CONFIG: Input<(), String> = Input::new("config");
/// The `lstat` of a directory or a file in the project.
pub static STAT: Input<String, Stamped> = Input::new("stat");
/// What replaces a file for a check of an edit before it is written: an input
/// of its own so that an overlay never looks like a change on disk.
pub static OVERLAY: Input<String, Option<Content>> = Input::new("overlay");

/// What a directory holds, ignored or not.
pub static ENTRIES: Query<String, Entries> = Query::new("entries", entries);
/// What a file holds, hashed once; an overlay stands in for the file.
pub static CONTENT: Query<String, Content> = Query::new("content", content::content);
/// What a file holds as text, in memory only.
pub static TEXT: Query<String, Option<String>> = Query::new("text", content::text).transient();
/// The Merkle hash of a directory.
pub static TREE: Query<String, Hash> = Query::new("tree", tree);
/// The files of the project, sorted.
pub static FILES: Query<(), Vec<String>> = Query::new("files", files);

/// A BLAKE3 hash.
pub type Hash = [u8; 32];

/// The `lstat` of a directory or a file the walk found in the project, with
/// the start of the walk when it is racy: too new to trust, as the `lstat` of
/// a file written within the clock's tick of the walk does not show a change
/// after it. The start is another value on every walk, so that what it
/// stands for is read again, and it is none once the file is old enough. None
/// for what the project no longer holds.
pub type Stamped = Option<(Stat, Option<i64>)>;

/// A child of a directory that is in the project.
struct Child {
    name: String,
    at: String,
    is_dir: bool,
}

fn entries(graph: &Graph, dir: &String) -> Result<Entries, Error> {
    graph.input(&STAT, dir)?;
    let root = graph.input(&ROOT, &())?.unwrap_or_default();
    Ok(read_entries(&Path::new(root.as_str()).join(dir)))
}

/// The Merkle hash of a directory: BLAKE3 over its children in the project,
/// sorted by name, each as (name, kind, hash): the content of a file, the
/// `tree` of a directory.
fn tree(graph: &Graph, dir: &String) -> Result<Hash, Error> {
    let children = children(graph, dir)?;
    let hash = |graph: &Graph, child: &Child| -> Result<Hash, Error> {
        if child.is_dir {
            Ok(*graph.query(&TREE, &child.at)?)
        } else {
            Ok(graph.query(&CONTENT, &child.at)?.blake3)
        }
    };
    let children = Arc::new(children);
    let hashes = if children.len() < FAN_OUT {
        children.iter().map(|child| hash(graph, child)).collect()
    } else {
        let order: Vec<usize> = (0..children.len()).collect();
        let shared = children.clone();
        graph.par_map(order, move |graph, i| hash(graph, &shared[*i]))
    };
    seal(&children, hashes)
}

fn seal(children: &[Child], hashes: Vec<Result<Hash, Error>>) -> Result<Hash, Error> {
    let mut hasher = blake3::Hasher::new();
    for (child, hash) in children.iter().zip(hashes) {
        hasher.update(&(child.name.len() as u64).to_le_bytes());
        hasher.update(child.name.as_bytes());
        hasher.update(&[u8::from(child.is_dir)]);
        hasher.update(&hash?);
    }
    Ok(hasher.finalize().into())
}

/// The project's files, sorted: every file the directories hold that is in
/// the project, below the root. It reads the ignore configuration so that a
/// change of it is a new list.
fn files(graph: &Graph, _: &()) -> Result<Vec<String>, Error> {
    graph.input(&CONFIG, &())?;
    let (mut files, mut todo) = (Vec::new(), vec![String::new()]);
    while let Some(dir) = todo.pop() {
        for child in children(graph, &dir)? {
            if child.is_dir {
                todo.push(child.at);
            } else {
                files.push(child.at);
            }
        }
    }
    // In the order of paths, which compares by component, as the walk does.
    files.sort_by(|a, b| Path::new(a).cmp(Path::new(b)));
    Ok(files)
}

/// The children of a directory the walk found in the project, by name: those
/// that have a `stat` set.
fn children(graph: &Graph, dir: &String) -> Result<Vec<Child>, Error> {
    let mut children = Vec::new();
    for raw in &graph.query(&ENTRIES, dir)?.raw {
        let name = raw.name.to_string_lossy().into_owned();
        let at = Path::new(dir)
            .join(&raw.name)
            .to_string_lossy()
            .into_owned();
        let found = graph.input(&STAT, &at)?.is_some_and(|stat| stat.is_some());
        if found && raw.kind != Kind::Other {
            children.push(Child {
                name,
                at,
                is_dir: raw.kind == Kind::Dir,
            });
        }
    }
    children.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(children)
}
