use lighthouse_model::Judgment;
use thiserror::Error;

/// Why a store operation failed.
#[derive(Debug, Error)]
pub enum Error {
    #[error("store: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("store: {0}")]
    Json(#[from] serde_json::Error),
    #[error("store: {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error(
        "{path} is not a usable database; nothing was deleted. Move it aside \
         (for example `mv {path} {path}.bak`) and run the command again: the cache is rebuilt \
         from the decision log, and only the history of sightings is lost"
    )]
    Corrupt { path: String },
    #[error("the decision log {path} line {line}: {reason}")]
    Log {
        path: String,
        line: usize,
        reason: String,
    },
    #[error("no recorded finding matches `{0}`")]
    UnknownFinding(String),
    #[error(
        "`{prefix}` matches several recorded findings; retry with a longer prefix. Candidates: {}",
        candidates.join(", ")
    )]
    AmbiguousFinding {
        prefix: String,
        /// At most five full fingerprints that start with the prefix.
        candidates: Vec<String>,
    },
    #[error(
        "the finding was seen again at {actual}, after {expected}; read it again before judging"
    )]
    Changed { expected: String, actual: String },
    #[error("a suppression goes with a `fail`, not with `{0}`")]
    SuppressionWithoutFail(Judgment),
}
