//! The host result cache: what a rule found for one subject, kept between runs
//! under a key of everything the finding depends on. The cache is local and
//! derived: it is never committed, a run can bypass it, `clean` deletes it, and
//! its size is capped, oldest use first. Any doubt is a miss.
//!
//! [`Digests`] hash the merged project once, per file and as a whole; a
//! [`Key`] is built from those and from the rule; [`Cache`] stores the
//! findings under it in `results.db`.

use std::{fs, io, path::Path};

use thiserror::Error;

mod digest;
mod key;
mod neighbors;
mod store;

pub use digest::{Digest, Digests, FileDigest, documents};
pub use key::{Key, KeyBuilder};
pub use store::{Cache, DEFAULT_LIMIT, Stats, Stored};

/// Name of the cache directory under `.lighthouse`.
pub const DIR: &str = "cache";

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
    #[error("the code model cannot be hashed: {0}")]
    Hash(#[from] serde_json::Error),
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
