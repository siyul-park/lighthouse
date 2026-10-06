use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Parse { path: String, message: String },
    /// Files that do not form a catalog: ordering, naming, missing files.
    #[error("{path}: {message}")]
    Layout { path: String, message: String },
    /// A pattern, option, source or overlay that breaks a catalog rule.
    #[error("`{id}`: {reason}")]
    Invalid { id: String, reason: String },
}

impl Error {
    pub(crate) fn invalid(id: &str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            id: id.to_owned(),
            reason: reason.into(),
        }
    }

    pub(crate) fn layout(path: &str, message: impl Into<String>) -> Self {
        Self::Layout {
            path: path.to_owned(),
            message: message.into(),
        }
    }
}
