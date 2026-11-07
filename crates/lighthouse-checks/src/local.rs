//! Queries over a model alone, with no graph: each index made from the model
//! on first use and each fact built when first asked for. It is what the
//! graph's answers are held to, and what a run without a graph reads.

use std::{
    any::Any,
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex, PoisonError},
};

use lighthouse_model::ucm::Model;
use lighthouse_plugin::{Error, Queries, Reads, Workspace};

use crate::{
    builder::{Over, subject},
    fact::{Built, FACT, Facts, class_of, fact_of},
    indexes::Computed,
};

/// The queries of a model, for the rules whose reads are known.
pub struct Local<'a> {
    model: &'a (dyn Model + Sync),
    ws: &'a Workspace,
    reads: BTreeMap<String, Reads>,
    indexes: Computed<'a>,
    facts: Mutex<BTreeMap<String, Arc<Facts>>>,
}

impl<'a> Local<'a> {
    /// The queries of `model` for what is read of each sort of subject, by the
    /// class of what is read.
    pub fn new(
        model: &'a (dyn Model + Sync),
        ws: &'a Workspace,
        reads: impl IntoIterator<Item = (String, Reads)>,
    ) -> Self {
        Self {
            model,
            ws,
            reads: reads
                .into_iter()
                .map(|(_, reads)| (class_of(&reads), reads))
                .collect(),
            indexes: Computed::new(model, ws),
            facts: Mutex::default(),
        }
    }

    fn built(&self, key: &str) -> Result<Arc<Facts>, Error> {
        if let Some(found) = self
            .facts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
        {
            return Ok(Arc::clone(found));
        }
        let (kind, class, file) = fact_of(key);
        let reads = self.reads.get(class).cloned().unwrap_or_default();
        let over = Over {
            model: self.model,
            indexes: &self.indexes,
            ws: self.ws,
        };
        let mut subjects = Vec::new();
        for symbol in self.model.symbols_in(Path::new(file)) {
            let fact = subject(kind, over, &reads, symbol)?;
            subjects.push((symbol.id.as_str().to_owned(), Built::of(fact)));
        }
        let built = Arc::new(Facts::of(subjects));
        self.facts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.to_owned(), Arc::clone(&built));
        Ok(built)
    }
}

impl Queries for Local<'_> {
    fn pull(&self, kind: &str, key: &str) -> Result<Arc<dyn Any + Send + Sync>, Error> {
        match kind {
            FACT => Ok(self.built(key)?),
            _ => self.indexes.pull(kind, key),
        }
    }
}
