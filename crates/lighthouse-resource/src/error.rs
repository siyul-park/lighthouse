use thiserror::Error;

/// Why a document could not be read as a resource.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// The text is not valid YAML, TOML or JSON.
    #[error("{path}: {message}")]
    Parse { path: String, message: String },
    /// The document has no `apiVersion`: it is in a format from before the
    /// resource model.
    #[error(
        "{path}: not a `{api_version}` resource (no `apiVersion`); run `lighthouse spec migrate`"
    )]
    Legacy { path: String, api_version: String },
    /// The document is a resource of another version or kind than asked for.
    #[error("{path}: expected {expected}, found {found}")]
    Mismatch {
        path: String,
        expected: String,
        found: String,
    },
    /// The document has the right kind but its content does not fit it.
    #[error("{path}: {message}")]
    Invalid { path: String, message: String },
}

impl Error {
    pub(crate) fn parse(path: &str, message: impl ToString) -> Self {
        Self::Parse {
            path: path.to_owned(),
            message: message.to_string(),
        }
    }
}
