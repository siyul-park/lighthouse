use std::{io, path::PathBuf};

use thiserror::Error;

/// Why a catalog or a project could not be read, built, validated or written;
/// every variant names the offending file, decision or project.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    /// A document that is not a valid resource of the kind it declares.
    #[error(transparent)]
    Document(#[from] lighthouse_resource::Error),
    /// Files that do not form a catalog: ordering, naming, missing files.
    #[error("{path}: {message}")]
    Layout { path: String, message: String },
    /// A decision, option, source or overlay that breaks a catalog rule.
    #[error("`{id}`: {reason}")]
    Invalid { id: String, reason: String },
    #[error("{path}: expected one Project document, found {found}")]
    Documents { path: String, found: usize },
    #[error("invalid glob `{pattern}`: {source}")]
    Glob {
        pattern: String,
        source: globset::Error,
    },
    #[error("unknown project `{0}`")]
    UnknownProject(String),
    #[error("project `{0}` extends itself")]
    ProjectCycle(String),
    #[error("project `{0}` is defined twice")]
    DuplicateProject(String),
    /// A project that says something it cannot mean.
    #[error("invalid configuration: {0}")]
    Config(String),
}

impl Error {
    /// What reading or writing `path` failed with.
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { path, source }
    }

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
