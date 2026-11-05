//! Setting the inputs of the graph from a walk.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lighthouse_graph::{Db, Durability};

use super::{CONFIG, Content, ENTRIES, OVERLAY, ROOT, STAT, Walked, key, racy};
use crate::{Error, Overlays, engine::Engine};

impl Engine {
    /// Sets every input of the walk: the root, the ignore configuration, the
    /// `stat` of each file (with the salt of the racy ones, returned for the
    /// next walk) and what each directory lists.
    pub(super) fn set_inputs(
        &self,
        db: &mut Db,
        state: &Walked,
        walk: &lighthouse_spec::Walked,
    ) -> Result<BTreeMap<String, u64>, Error> {
        let root = self.ws.root.to_string_lossy().into_owned();
        db.set_input(&ROOT, &(), root, Durability::Static)?;
        let ignores = blake3::hash(self.ws.ignores.join("\n").as_bytes())
            .to_hex()
            .to_string();
        db.set_input(&CONFIG, &(), ignores, Durability::Stable)?;
        let walks = state.walks + 1;
        let mut salts = state.salts.clone();
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
        Ok(salts)
    }
}

/// Sets the overlays, and unsets those of the last run that are gone.
pub(super) fn set_overlays(
    db: &mut Db,
    before: &[String],
    overlays: &Overlays,
) -> Result<(), Error> {
    let overlaid: BTreeSet<String> = overlays.keys().map(|p| key(p)).collect();
    for (path, text) in overlays {
        let content = Some(Content::of_text(text));
        db.set_input(&OVERLAY, &key(path), content, Durability::Volatile)?;
    }
    for gone in before.iter().filter(|p| !overlaid.contains(*p)) {
        db.set_input(&OVERLAY, gone, None, Durability::Volatile)?;
    }
    Ok(())
}
