//! The detect stage: walk, set the inputs, pull what the run needs.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::PoisonError,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use lighthouse_spec::Listing;

use super::{
    Content,
    inputs::{Reading, set_overlays, set_root, set_stats},
    pull::{Pulled, pull},
};
use crate::{
    Error, Overlays,
    engine::{Engine, in_pool},
    timings::Detect,
};

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
        let opened = guard.get_or_insert_with(|| self.open_graph());
        let mut notices: Vec<String> = opened.notice.take().into_iter().collect();
        let started = now_ns();
        opened.graph.take_executed();

        set_root(&mut opened.graph, &self.ws.root, &self.ws.ignores)?;
        opened.overlaid = set_overlays(&mut opened.graph, &opened.overlaid, overlays)?;
        let mut reading = Reading {
            graph: &mut opened.graph,
            root: self.ws.root.clone(),
            started,
            dirs: Default::default(),
            texts: Default::default(),
        };
        let walk = self.files.walk(&mut reading);
        let seen = (reading.dirs, reading.texts);
        opened.statted = set_stats(
            &mut opened.graph,
            started,
            &opened.statted,
            seen,
            &walk.stats,
        )?;

        let Pulled {
            files,
            tree,
            contents,
        } = in_pool(|| pull(&opened.graph, &wanted))?;

        let executed = opened.graph.take_executed();
        let ran = |kind: &'static str| executed.iter().filter(move |label| label.starts_with(kind));
        let read = ran("content(")
            .filter(|label| {
                !opened
                    .overlaid
                    .iter()
                    .any(|p| **label == format!("content({p:?})"))
            })
            .count();
        let entries = ran("entries(").count();
        let mut listing = walk.listing;
        debug_assert_eq!(
            listing.files,
            files.iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        listing.files = files.iter().map(PathBuf::from).collect();

        let committing = Instant::now();
        if let Err(e) = opened.graph.commit() {
            notices.push(format!("the graph was not saved: {e}"));
        }
        let timing = Detect {
            stat: walk.stats.len(),
            read,
            entries,
            tree: tree.iter().map(|b| format!("{b:02x}")).collect(),
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
