//! Queries over a model alone, with no graph: each index made from the model
//! on first use and each fact built when first asked for. It is what the
//! graph's answers are held to, and what a run without a graph reads.

use std::{
    any::Any,
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

use lighthouse_model::ucm::{Model, SymbolId};
use lighthouse_plugin::{Error, Queries, Reads, Workspace};

use crate::{
    builder::{Over, subject},
    fact::{Built, FACT, subject_of},
    indexes::{
        CREDITED, CYCLES, Computed, IMPORTS, Indexes, NAMES, OWNED, PARENTS, REACH, SPLIT,
        STABILITY, TAKERS, TESTED, TYPES, USERS, credit_of,
    },
};

/// The queries of a model, for the rules whose reads are known.
pub struct Local<'a> {
    model: &'a (dyn Model + Sync),
    ws: &'a Workspace,
    reads: BTreeMap<String, Reads>,
    indexes: Computed<'a>,
    facts: Mutex<BTreeMap<String, Arc<Built>>>,
}

impl<'a> Local<'a> {
    /// The queries of `model` for what is read of each sort of subject.
    pub fn new(
        model: &'a (dyn Model + Sync),
        ws: &'a Workspace,
        reads: impl IntoIterator<Item = (String, Reads)>,
    ) -> Self {
        Self {
            model,
            ws,
            reads: reads.into_iter().collect(),
            indexes: Computed::new(model, ws),
            facts: Mutex::default(),
        }
    }

    fn built(&self, key: &str) -> Result<Arc<Built>, Error> {
        if let Some(found) = self
            .facts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
        {
            return Ok(Arc::clone(found));
        }
        let (kind, id) = subject_of(key);
        let fact = SymbolId::parse(id)
            .and_then(|id| self.model.symbol(&id))
            .map(|symbol| {
                let over = Over {
                    model: self.model,
                    indexes: &self.indexes,
                    ws: self.ws,
                };
                subject(
                    kind,
                    over,
                    &self.reads.get(kind).cloned().unwrap_or_default(),
                    symbol,
                )
            })
            .transpose()?
            .flatten();
        let built = Arc::new(Built::of(fact));
        self.facts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.to_owned(), Arc::clone(&built));
        Ok(built)
    }
}

impl Queries for Local<'_> {
    fn pull(&self, kind: &str, key: &str) -> Result<Arc<dyn Any + Send + Sync>, Error> {
        let id = || {
            SymbolId::parse(key).unwrap_or_else(|| {
                SymbolId::new(key, &[], "", lighthouse_model::ucm::SymbolKind::Function)
            })
        };
        let indexes = &self.indexes;
        Ok(match kind {
            FACT => self.built(key)?,
            TAKERS => Arc::new(indexes.takers(key)),
            USERS => Arc::new(indexes.users(&id())),
            OWNED => Arc::new(indexes.owned(&id())),
            NAMES => Arc::new(indexes.names(key)),
            IMPORTS => Arc::new(indexes.imports(key)),
            CYCLES => Arc::new(indexes.cycle(key)),
            TYPES => Arc::new(indexes.types(key)),
            PARENTS => Arc::new(indexes.parent(key)),
            SPLIT => Arc::new(indexes.split(&id())),
            REACH => Arc::new(indexes.reach(&id())),
            STABILITY => Arc::new(indexes.stability(key)),
            TESTED => Arc::new(indexes.tested(key)),
            CREDITED => {
                let (prefix, modules) = credit_of(key);
                Arc::new(indexes.credited(prefix, &modules).as_ref().clone())
            }
            other => return Err(Error::Failed(format!("`{other}` is no query of a model"))),
        })
    }
}
