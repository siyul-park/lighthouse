//! The detect stage: walk, set the inputs, pull what the run needs.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::PoisonError,
    time::Instant,
};

use lighthouse_spec::{Content, Listing};

use super::pull::{Pulled, pull};
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
        let mut guard = self.lock_graph();
        let mut notices = Vec::new();
        let graph = self.graph_in(&mut guard, &mut notices);
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        graph.take_executed();

        let walk = self.files.walk(graph, &mut held, overlays)?;

        let Pulled {
            files,
            tree,
            contents,
        } = in_pool(|| pull(graph, &wanted))?;

        let executed = graph.take_executed();
        let ran = |kind: &'static str| executed.iter().filter(move |label| label.starts_with(kind));
        let read = ran("content(")
            .filter(|label| {
                !held
                    .overlaid()
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
        if let Err(e) = graph.commit() {
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
