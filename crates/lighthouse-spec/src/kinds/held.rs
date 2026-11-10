//! The paths a process set an input for, so that the next walk can unset
//! what the project no longer has.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use lighthouse_graph::{Durability, Error, Graph};

use super::{Content, OVERLAY, ROOT, VIRTUAL, key};

/// The paths this process set an `overlay` and a `stat` for, at the last
/// walk.
#[derive(Debug, Default)]
pub struct Held {
    overlaid: BTreeSet<String>,
    /// The directories an overlay adds a file to.
    added: BTreeSet<String>,
    pub(crate) statted: BTreeSet<String>,
}

impl Held {
    /// The paths that have an overlay.
    pub fn overlaid(&self) -> &BTreeSet<String> {
        &self.overlaid
    }

    /// Sets the overlays, and unsets those of the last walk that are gone. An
    /// overlay for a file the file system does not have, in a directory it
    /// does, adds that file to the directory.
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
        self.add_files(graph, overlays)
    }

    /// Sets the files the overlays add to their directories, and empties the
    /// directories of the last walk that no overlay adds to now.
    fn add_files(
        &mut self,
        graph: &mut Graph,
        overlays: &BTreeMap<PathBuf, String>,
    ) -> Result<(), Error> {
        let root = graph.input(&ROOT, &())?.unwrap_or_default();
        let mut added: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in overlays.keys() {
            let (Some(name), Some(parent)) = (path.file_name(), path.parent()) else {
                continue;
            };
            let there = Path::new(root.as_str()).join(parent);
            let new = std::fs::symlink_metadata(Path::new(root.as_str()).join(path)).is_err();
            if new && there.is_dir() {
                added
                    .entry(key(parent))
                    .or_default()
                    .push(name.to_string_lossy().into_owned());
            }
        }
        for (dir, names) in &added {
            graph.set_input(&VIRTUAL, dir, names.clone(), Durability::Volatile)?;
        }
        for gone in self.added.iter().filter(|dir| !added.contains_key(*dir)) {
            graph.set_input(&VIRTUAL, gone, Vec::new(), Durability::Volatile)?;
        }
        // Reading the directory's additions registers the kind, as above.
        graph.input(&VIRTUAL, &String::new())?;
        self.added = added.into_keys().collect();
        Ok(())
    }
}
