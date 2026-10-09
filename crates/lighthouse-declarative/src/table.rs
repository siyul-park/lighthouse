//! The facts of a symbol that every check of a run reads alike, built once and
//! shared by all checks, in the form the CEL runtime holds a variable.

use std::{
    collections::HashMap,
    hash::{BuildHasher, RandomState},
    sync::{Arc, OnceLock, PoisonError, RwLock},
};

use lighthouse_model::SymbolId;

use crate::eval::Fact;

const SHARDS: usize = 32;

type Cell = Arc<OnceLock<Option<Fact>>>;

/// Facts by symbol, sharded so that checks running in parallel rarely wait
/// for each other; a fact is built by whichever check asks first.
pub(crate) struct Cache {
    hasher: RandomState,
    shards: Vec<RwLock<HashMap<SymbolId, Cell>>>,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            hasher: RandomState::new(),
            shards: (0..SHARDS).map(|_| RwLock::default()).collect(),
        }
    }
}

impl Cache {
    /// Reads the fact of `id` with `read`, building it with `build` first if no
    /// check has yet; `None` when the symbol has no fact of this kind.
    pub(crate) fn with<R>(
        &self,
        id: &SymbolId,
        build: impl FnOnce() -> Option<Fact>,
        read: impl FnOnce(&Fact) -> R,
    ) -> Option<R> {
        self.cell(id).get_or_init(build).as_ref().map(read)
    }

    fn cell(&self, id: &SymbolId) -> Cell {
        let shard = &self.shards[self.hasher.hash_one(id) as usize % SHARDS];
        if let Some(cell) = shard.read().unwrap_or_else(PoisonError::into_inner).get(id) {
            return Arc::clone(cell);
        }
        let mut shard = shard.write().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(shard.entry(id.clone()).or_default())
    }
}

/// The base facts of the symbols a run's checks select, by selected kind.
#[derive(Default)]
pub(crate) struct Table {
    pub(crate) symbols: Cache,
    pub(crate) functions: Cache,
    pub(crate) tests: Cache,
}
