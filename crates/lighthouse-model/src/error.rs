use thiserror::Error;

/// Why a resource could not be read or written.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// A file could not be safely written.
    #[error("cannot write {path}: {message}")]
    Io { path: String, message: String },
    /// The text is not valid YAML, TOML or JSON.
    #[error("{path}: {message}")]
    Parse { path: String, message: String },
    /// The document is a resource of another version or kind than asked for,
    /// or declares none (`(none)`).
    #[error("{path}: expected {expected}, found {found}")]
    Mismatch {
        path: String,
        expected: String,
        found: String,
    },
    /// The document has the right kind but its content does not fit it.
    #[error("{path}: {message}")]
    Invalid { path: String, message: String },
    /// The name of a listed document could not be found in its text.
    #[error("{path}: could not find where `{name}` is written")]
    Unplaced { path: String, name: String },
}

impl Error {
    pub(crate) fn parse(path: &str, message: impl ToString) -> Self {
        Self::Parse {
            path: path.to_owned(),
            message: message.to_string(),
        }
    }
}
