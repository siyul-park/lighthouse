use std::path::PathBuf;

use thiserror::Error;

/// Why discovering, starting or registering an external plugin failed. Every
/// variant but `Unavailable` is a configuration error that aborts the run.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{}: {message}", path.display())]
    Manifest { path: PathBuf, message: String },
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
