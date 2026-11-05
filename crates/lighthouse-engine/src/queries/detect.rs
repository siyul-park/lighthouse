//! The detect stage: walk, set the inputs, pull what the run needs.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::PoisonError,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use lighthouse_graph::{Db, Durability};
use lighthouse_spec::Listing;

use super::{
    CONFIG, CONTENT, Content, DIR, ENTRIES, FILES, OVERLAY, ROOT, STAT, Walked, racy, register,
};
use crate::{
    Error, Overlays, cache,
    engine::{Engine, in_pool},
    timings::Detect,
};

/// The file of the graph under the cache directory.
const DB: &str = "graph.db";
/// Where the last walk is kept in the graph's store.
const WALK: &str = "walk";

/// The open graph of an engine, and what a run knows of it.
pub(crate) struct Graph {
    db: Db,
    notice: Option<String>,
}

/// What detect found: the listing, the content of the files `wanted`, and
/// where the time went.
pub(crate) struct Detected {
    pub listing: Listing,
    pub contents: BTreeMap<PathBuf, Content>,
    pub timing: Detect,
    pub notices: Vec<String>,
}

impl Engine {
    /// Lists the project and has the content of the files `wanted` hashed.
    /// `overlays` stand in for the files of the same path.
    pub(crate) fn detect(
        &self,
        overlays: &Overlays,
        wanted: impl Fn(&Path) -> bool + Sync,
    ) -> Result<Detected, Error> {
        let mut guard = self.graph.lock().unwrap_or_else(PoisonError::into_inner);
        let graph = guard.get_or_insert_with(|| self.open_graph());
        let mut notices: Vec<String> = graph.notice.take().into_iter().collect();
        let db = &mut graph.db;
        db.take_executed();

        let state: Walked = db
            .state(WALK)?
            .and_then(|bytes| postcard::from_bytes(&bytes).ok())
            .unwrap_or_default();
        let started = now_ns();
        let walks = state.walks + 1;
        let walk = self.files.walk(&state.dirs, state.started);

        let root = self.ws.root.to_string_lossy().into_owned();
        db.set_input(&ROOT, &(), root, Durability::Static)?;
        let ignores = blake3::hash(self.ws.ignores.join("\n").as_bytes())
            .to_hex()
            .to_string();
        db.set_input(&CONFIG, &(), ignores, Durability::Stable)?;
        let mut salts = state.salts;
        for (rel, stat) in &walk.stats {
            let at = key(rel);
            if racy(stat.mtime_ns, state.started) {
                salts.insert(at.clone(), walks);
            }
            let salt = salts.get(&at).copied().unwrap_or(0);
            db.set_input(&STAT, &at, (*stat, salt), Durability::Volatile)?;
        }
        salts.retain(|at, _| walk.stats.contains_key(Path::new(at)));
        for (dir, children) in &walk.listed {
            let names = children
                .iter()
                .map(|c| (c.name.to_string_lossy().into_owned(), c.is_dir))
                .collect();
            db.set_input(&ENTRIES, &key(dir), names, Durability::Volatile)?;
        }
        let overlaid: BTreeSet<String> = overlays.keys().map(|p| key(p)).collect();
        for (path, text) in overlays {
            let content = Some(Content::of_text(text));
            db.set_input(&OVERLAY, &key(path), content, Durability::Volatile)?;
        }
        for gone in state.overlaid.iter().filter(|p| !overlaid.contains(*p)) {
            db.set_input(&OVERLAY, gone, None, Durability::Volatile)?;
        }

        let db = &*db;
        let (files, root_hash, contents) = in_pool(|| -> Result<_, Error> {
            let files = db.query(&FILES, &())?;
            let root_hash = *db.query(&DIR, &String::new())?;
            let claimed: Vec<PathBuf> = files
                .iter()
                .map(PathBuf::from)
                .filter(|rel| wanted(rel))
                .collect();
            let pulled = db.par_map(claimed.clone(), |db, rel| {
                db.query(&CONTENT, &key(rel))
                    .map(|content| content.as_ref().clone())
            });
            let contents: Result<BTreeMap<PathBuf, Content>, Error> = claimed
                .into_iter()
                .zip(pulled)
                .map(|(rel, content)| Ok((rel, content?)))
                .collect();
            Ok((files, root_hash, contents?))
        })?;

        let executed = db.take_executed();
        let read = executed
            .iter()
            .filter(|label| label.starts_with("content("))
            .filter(|label| {
                !overlaid
                    .iter()
                    .any(|p| **label == format!("content({p:?})"))
            })
            .count();
        let mut listing = walk.listing;
        debug_assert_eq!(
            listing.files,
            files.iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        listing.files = files.iter().map(PathBuf::from).collect();

        let walked = Walked {
            started: Some(started),
            walks,
            salts,
            dirs: walk.dirs,
            overlaid: overlaid.into_iter().collect(),
        };
        let committing = Instant::now();
        let bytes = postcard::to_stdvec(&walked).map_err(|e| Error::Fix(e.to_string()))?;
        graph.db.set_state(WALK, bytes);
        if let Err(e) = graph.db.commit() {
            notices.push(format!("the graph was not saved: {e}"));
        }
        let timing = Detect {
            stat: walk.stats.len(),
            read,
            relisted: walk.relisted,
            root: root_hash.iter().map(|b| format!("{b:02x}")).collect(),
            commit: committing.elapsed(),
        };
        Ok(Detected {
            listing,
            contents,
            timing,
            notices,
        })
    }

    /// Opens `graph.db` beside the result cache; without a cache, or when it
    /// cannot be opened, the graph lives in memory.
    fn open_graph(&self) -> Graph {
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

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos() as i64)
}
