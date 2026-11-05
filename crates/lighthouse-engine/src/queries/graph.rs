//! The graph of an engine: opened by the first run, beside the result cache.

use lighthouse_graph::Db;

use super::register;
use crate::{cache, engine::Engine};

/// The file of the graph under the cache directory.
const DB: &str = "graph.db";

/// The open graph of an engine, and what a run knows of it.
pub(crate) struct Graph {
    pub(super) db: Db,
    pub(super) notice: Option<String>,
}

impl Engine {
    /// Opens `graph.db` beside the result cache; without a cache, or when it
    /// cannot be opened, the graph lives in memory.
    pub(super) fn open_graph(&self) -> Graph {
        let versions = [("binary".to_owned(), cache::build_identity())].into();
        let opened = match &self.cache {
            Some(cache) => Db::open(&cache.dir.join(DB), &versions).map(|mut db| {
                db.set_limit(cache.limit);
                (db, None)
            }),
            None => Db::in_memory(&versions).map(|db| (db, None)),
        };
        let (db, notice) = opened.unwrap_or_else(|e| {
            let db = Db::in_memory(&versions).expect("an in-memory graph opens");
            (db, Some(format!("the graph is not kept: {e}")))
        });
        register(&db);
        Graph { db, notice }
    }
}
