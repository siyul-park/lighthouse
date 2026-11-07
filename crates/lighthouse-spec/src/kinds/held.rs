//! The paths a process set an input for, so that the next walk can unset
//! what the project no longer has.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_graph::{Durability, Error, Graph};

use super::{Content, OVERLAY, key};

/// The paths this process set an `overlay` and a `stat` for, at the last
/// walk.
#[derive(Debug, Default)]
pub struct Held {
    overlaid: BTreeSet<String>,
    pub(crate) statted: BTreeSet<String>,
}

impl Held {
    /// The paths that have an overlay.
    pub fn overlaid(&self) -> &BTreeSet<String> {
        &self.overlaid
    }

    /// Sets the overlays, and unsets those of the last walk that are gone.
    pub fn set_overlays(
        &mut self,
        graph: &mut Graph,
        overlays: &BTreeMap<PathBuf, String>,
    ) -> Result<(), Error> {
        let overlaid: BTreeSet<String> = overlays.keys().map(|p| key(p)).collect();
        for (path, text) in overlays {
            let content = Some(Content::of_text(text));
            graph.set_input(&OVERLAY, &key(path), content, Durability::Volatile)?;
        }
        for gone in self.overlaid.iter().filter(|p| !overlaid.contains(*p)) {
            graph.set_input(&OVERLAY, gone, None, Durability::Volatile)?;
        }
        // Reading an overlay registers its kind, which a stored `content` that
        // read none depends on, whether or not one is set.
        graph.input(&OVERLAY, &String::new())?;
        self.overlaid = overlaid;
        Ok(())
    }
}
