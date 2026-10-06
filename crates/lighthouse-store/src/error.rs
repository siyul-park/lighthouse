use lighthouse_model::MismatchedReason;
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
        "the store was written by a newer lighthouse (schema {found}, this build knows {supported})"
    )]
    NewerSchema { found: usize, supported: usize },
    #[error("no recorded finding matches `{0}`")]
    UnknownFinding(String),
    #[error("`{0}` matches several recorded findings; give more of the fingerprint")]
    AmbiguousFinding(String),
    #[error(transparent)]
    Reason(#[from] MismatchedReason),
}
