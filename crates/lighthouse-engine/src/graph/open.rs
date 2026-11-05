//! The graph of an engine: opened by the first run, beside the result cache.

use std::collections::BTreeSet;

use lighthouse_graph::{Graph, Options};

use crate::{cache, engine::Engine};

/// The file of the graph under the cache directory.
const FILE: &str = "graph.db";

/// The open graph of an engine, and what a run knows of it.
pub(crate) struct Opened {
    pub(crate) graph: Graph,
    pub(super) notice: Option<String>,
    /// The paths this process set an overlay for, at the last run.
    pub(super) overlaid: BTreeSet<String>,
    /// The paths this process set a `stat` for, at the last run.
    pub(super) statted: BTreeSet<String>,
}

impl Engine {
    /// Opens `graph.db` beside the result cache; without a cache, or when it
    /// cannot be opened, the graph lives in memory.
    pub(crate) fn open_graph(&self) -> Opened {
        let options = |limit| Options {
            versions: [("binary".to_owned(), cache::build_identity())].into(),
            limit,
            ..Options::default()
        };
        let opened = match &self.cache {
            Some(cache) => {
                Graph::open(&cache.dir.join(FILE), options(cache.limit)).map(|graph| (graph, None))
            }
            None => Graph::in_memory(options(lighthouse_graph::DEFAULT_LIMIT))
                .map(|graph| (graph, None)),
        };
        let (graph, notice) = opened.unwrap_or_else(|e| {
            let graph = Graph::in_memory(options(lighthouse_graph::DEFAULT_LIMIT))
                .expect("an in-memory graph opens");
            (graph, Some(format!("the graph is not kept: {e}")))
        });
        Opened {
            graph,
            notice,
            overlaid: BTreeSet::new(),
            statted: BTreeSet::new(),
        }
    }
}
