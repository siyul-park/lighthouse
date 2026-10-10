use std::{collections::BTreeMap, path::PathBuf};

use crate::Position;

/// A document of the project that is not code, such as a decision of a
/// catalog: a thing with a kind, a name and labels, written at a line of a
/// file. It is the first subject of the model that no language provider
/// states; the engine finds the documents itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub file: PathBuf,
    /// Where the name is written.
    pub at: Position,
    /// The document's `kind`, such as `Decision`.
    pub kind: String,
    /// The document's `metadata.name`.
    pub name: String,
    /// The document's `metadata.uid`, when it has one.
    pub uid: Option<String>,
    pub labels: BTreeMap<String, String>,
}
