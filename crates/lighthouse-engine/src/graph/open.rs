//! The graph of an engine: opened by the first use, beside the result cache.

use std::sync::{Arc, PoisonError};

use lighthouse_graph::{Graph, Options};
use lighthouse_plugin::Analyzers;
use lighthouse_spec::TEXT;

use crate::{cache, engine::Engine};

/// The file of the graph under the cache directory.
const FILE: &str = "graph.db";

impl Engine {
    /// The graph in `slot`, opened by the first call: `graph.db` beside the
    /// result cache; without a cache, or when it cannot be opened, the graph
    /// lives in memory, and a notice says so.
    pub(crate) fn graph_in<'g>(
        &self,
        slot: &'g mut Option<Graph>,
        notices: &mut impl Extend<String>,
    ) -> &'g mut Graph {
        slot.get_or_insert_with(|| {
            let options = |limit| {
                Options {
                    versions: [("binary".to_owned(), cache::build_identity())].into(),
                    limit,
                    ..Options::default()
                }
                .data(Arc::new(Analyzers::new(self.registry.analyzers(), text)))
            };
            let opened = match &self.cache {
                Some(cache) => Graph::open(&cache.dir.join(FILE), options(cache.limit)),
                None => Graph::in_memory(options(lighthouse_graph::DEFAULT_LIMIT)),
            };
            opened.unwrap_or_else(|e| {
                notices.extend([format!("the graph is not kept: {e}")]);
                Graph::in_memory(options(lighthouse_graph::DEFAULT_LIMIT))
                    .expect("an in-memory graph opens")
            })
        })
    }

    /// Locks the slot of the graph.
    pub(crate) fn lock_graph(&self) -> std::sync::MutexGuard<'_, Option<Graph>> {
        self.graph.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The text of a file as a query of the graph.
fn text(graph: &Graph, path: &str) -> Result<Arc<Option<String>>, lighthouse_graph::Error> {
    graph.query(&TEXT, &path.to_owned())
}
