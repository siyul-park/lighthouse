use thiserror::Error;

/// Why a catalog could not be read, built, validated or written; every
/// variant names the offending file or decision.
#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Parse { path: String, message: String },
    /// A document that is not a valid resource of the kind it declares.
    #[error(transparent)]
    Document(#[from] lighthouse_resource::Error),
    /// Files that do not form a catalog: ordering, naming, missing files.
    #[error("{path}: {message}")]
    Layout { path: String, message: String },
    /// A project directory in a format from before the resource model.
    #[error("{path}: a format from before the resource model; run `lighthouse spec migrate`")]
    Legacy { path: String },
    /// A decision, option, source or overlay that breaks a catalog rule.
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
