//! The host result cache: what models answered and what fixes proposed, kept
//! between runs under a key of everything they depend on. The findings of the
//! rules are nodes of the graph, not rows here. The cache is local and derived:
//! it is never committed, a run can bypass it, `clean` deletes it, and its size
//! is kept under one size limit, see [`fit`]. Any doubt is a miss.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use lighthouse_model::STATE_DIR;
use thiserror::Error;

mod fit;
mod key;
mod plugins;
mod store;

pub(crate) use fit::fit;
pub(crate) use key::digest_of;
pub use key::{Digest, KeyBuilder};
pub use store::{Cache, DEFAULT_LIMIT, Stats};

/// Name of the cache directory under `.lighthouse`.
pub const DIR: &str = "cache";

/// The directory of the cache that holds the baselines of editing sessions:
/// not derived, and aged out by the session. Any other directory of the cache
/// is a plugin's.
pub const HOOKS: &str = "hooks";

/// Why the cache could not be opened or written. A run goes on without it.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{}: {source}", path.display())]
    Io {
        path: std::path::PathBuf,
        source: io::Error,
    },
    #[error("a value cannot be hashed: {0}")]
    Hash(#[from] serde_json::Error),
    #[error("{0}")]
    Graph(#[from] lighthouse_graph::Error),
}

/// Where the cache of the project at `root` lives: `<root>/.lighthouse/cache`.
pub fn dir(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join(DIR)
}

/// What tells one build of Lighthouse from another: its version and the size
/// and modification time of the running program, so that results stored by
/// code that has since changed are never used.
pub fn build_identity() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let built = std::env::current_exe()
        .and_then(fs::metadata)
        .ok()
        .and_then(|meta| {
            let modified = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?;
            Some(format!("{}:{}", meta.len(), modified.as_nanos()))
        });
    // Without the program's own file there is nothing to tell builds by: a
    // key that no other run can have.
    built.map_or_else(
        || format!("{version}:unknown:{:?}", std::time::SystemTime::now()),
        |built| format!("{version}:{built}"),
    )
}

/// Deletes the cache directory entirely, whatever else has been put in it.
/// A directory that does not exist is already clean; returns whether there was
/// one.
pub fn clean(dir: &Path) -> Result<bool, Error> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(Error::Io {
            path: dir.to_owned(),
            source,
        }),
    }
}
