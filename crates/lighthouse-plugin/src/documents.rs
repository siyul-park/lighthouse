//! The project's own documents, and what the engine knows of each beyond its
//! identity: facts a rule reads as fields of the document (`decision.conflicts`)
//! that no file states and the engine works out only for a run that asks.

use std::collections::BTreeMap;

use lighthouse_model::Document;
use serde_json::Value;

/// No documents, for a run that found none.
pub static NO_DOCUMENTS: Documents = Documents::none();

/// The documents a run found and the facts the engine worked out for them.
#[derive(Debug, Default)]
pub struct Documents {
    list: Vec<Document>,
    facts: BTreeMap<String, Value>,
}

impl Documents {
    /// `list` with the facts of each, by the key of its document
    /// ([`Documents::key`]).
    pub fn new(list: Vec<Document>, facts: BTreeMap<String, Value>) -> Self {
        Self { list, facts }
    }

    /// No documents.
    pub const fn none() -> Self {
        Self {
            list: Vec::new(),
            facts: BTreeMap::new(),
        }
    }

    /// The documents, in file order.
    pub fn list(&self) -> &[Document] {
        &self.list
    }

    /// What the engine worked out for `document`, an object of fields to merge
    /// into its facts.
    pub fn facts_of(&self, document: &Document) -> Option<&Value> {
        self.facts.get(Self::key(document))
    }

    /// The identity of a document: its uid, else its name, so that a document
    /// is the same one under another name.
    pub fn key(document: &Document) -> &str {
        document.uid.as_deref().unwrap_or(&document.name)
    }
}
