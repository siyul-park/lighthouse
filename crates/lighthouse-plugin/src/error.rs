use std::fmt;

use thiserror::Error;

/// Why registering a plugin, resolving analyzers, reading facts or running a
/// rule failed. Messages name the offending id and are fit to show the user.
#[derive(Debug, Error)]
pub enum Error {
    #[error("`{0}` is registered twice")]
    Duplicate(String),
    #[error("the alias `{alias}` names the languages `{first}` and `{second}`")]
    Alias {
        alias: String,
        first: String,
        second: String,
    },
    #[error("`{id}` must be prefixed with `{plugin}/`")]
    Prefix { plugin: String, id: String },
    #[error("`{required_by}` requires unknown analyzer `{id}`")]
    MissingAnalyzer { id: String, required_by: String },
    #[error("analyzer cycle: {}", .0.join(" -> "))]
    Cycle(Vec<String>),
    #[error("invalid options for `{rule}`: {message}")]
    Options { rule: String, message: String },
    #[error("missing fact from `{0}`")]
    MissingFact(String),
    #[error("fact from `{analyzer}` has an unexpected shape: {message}")]
    BadFact { analyzer: String, message: String },
    #[error("{0}")]
    Failed(String),
    /// The check could not run to the end (a program crashed, timed out or
    /// exited with an error): what it covers was not checked, which is never
    /// the same as passing. The run reports the gap and goes on.
    #[error("{0}")]
    Incomplete(String),
}

impl Error {
    /// A failure of the model `id`, for the user to read.
    pub fn model(id: &str, reason: impl fmt::Display) -> Self {
        Self::Failed(format!("model `{id}`: {reason}"))
    }
}
