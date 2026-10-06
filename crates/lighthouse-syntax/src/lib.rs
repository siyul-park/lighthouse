mod node;
mod query;
mod registry;

pub use node::{children_named, descendants, span, text};
pub use query::{Match, Query};
pub use registry::Syntax;
pub use tree_sitter::{Language, Node, Tree};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("no grammar registered for `{0}`")]
    UnknownLanguage(String),
    #[error("grammar for `{0}` is incompatible with the parser")]
    Incompatible(String),
    #[error("parsing was interrupted")]
    Interrupted,
    #[error("invalid query: {0}")]
    Query(String),
}
