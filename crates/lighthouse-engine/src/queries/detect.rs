//! The detect stage: walk, set the inputs, pull what the run needs.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::PoisonError,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use lighthouse_spec::Listing;

use lighthouse_graph::{set_state, state};

use super::{
    Content, Walked,
    inputs::set_overlays,
    key,
    pull::{Pulled, pull},
};
use crate::{
    Error, Overlays,
    engine::{Engine, in_pool},
    timings::Detect,
};

/// Where the last walk is kept in the graph's store.
pub(super) const WALK: &str = "walk";

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

        let state: Walked = state(db, WALK)?
            .and_then(|bytes| postcard::from_bytes(&bytes).ok())
            .unwrap_or_default();
        let started = now_ns();
        let walks = state.walks + 1;
        let walk = self.files.walk(&state.dirs, state.started);

        let salts = self.set_inputs(db, &state, &walk)?;
        set_overlays(db, &state.overlaid, overlays)?;
        let overlaid: BTreeSet<String> = overlays.keys().map(|p| key(p)).collect();
        let Pulled {
            files,
            root: root_hash,
            contents,
        } = in_pool(|| pull(db, &wanted))?;

        let db = &*db;
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
        set_state(&mut graph.db, WALK, bytes);
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
}

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos() as i64)
}
