//! The graph of an engine: opened by the first use, beside the result cache.

use std::sync::{Arc, PoisonError};

use lighthouse_graph::{Graph, Options};
use lighthouse_plugin::Runtime;

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
            let options = || {
                Options {
                    versions: [("binary".to_owned(), cache::build_identity())].into(),
                    ..Options::default()
                }
                .data(Arc::new(Runtime::new(
                    self.registry.clone(),
                    (text, overlaid),
                    (self.ws.clone(), self.trusted),
                )))
            };
            let opened = match &self.cache {
                Some(cache) => Graph::open(&cache.dir.join(FILE), options()),
                None => Graph::in_memory(options()),
            };
            opened.unwrap_or_else(|e| {
                notices.extend([format!("the graph is not kept: {e}")]);
                Graph::in_memory(options()).expect("an in-memory graph opens")
            })
        })
    }

    /// Locks the slot of the graph.
    pub(crate) fn lock_graph(&self) -> std::sync::MutexGuard<'_, Option<Graph>> {
        self.graph.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The text of a file, read through the graph.
fn text(graph: &Graph, path: &str) -> Result<Arc<Option<String>>, lighthouse_graph::Error> {
    lighthouse_spec::text(graph, path).map(Arc::new)
}

/// The text a run stands in place of a file, read through the graph.
fn overlaid(graph: &Graph, path: &str) -> Result<Option<String>, lighthouse_graph::Error> {
    Ok(graph
        .input(&lighthouse_spec::OVERLAY, &path.to_owned())?
        .and_then(|content| content.as_ref().clone())
        .and_then(|content| content.text))
}
