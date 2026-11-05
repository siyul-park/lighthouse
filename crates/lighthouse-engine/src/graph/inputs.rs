//! Setting the inputs of the graph from a walk.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use lighthouse_graph::{Durability, Graph};
use lighthouse_spec::{Entries, Source, Stat};

use super::{CONFIG, CONTENT, Content, ENTRIES, OVERLAY, ROOT, STAT, Stamped, key};
use crate::{Error, Overlays};

/// What a walk is given by the graph: a directory is a `stat` input and the
/// `entries` query on it, the text of an ignore file is its `content`. It
/// keeps the paths it set a `stat` for.
pub(super) struct Reading<'a> {
    pub graph: &'a mut Graph,
    pub root: PathBuf,
    pub started: i64,
    pub dirs: BTreeSet<String>,
    pub texts: BTreeSet<String>,
}

impl Reading<'_> {
    fn stat(&mut self, at: &str, stat: Stat) -> Result<(), Error> {
        let stamped = stamp(stat, self.started);
        self.graph
            .set_input(&STAT, &at.to_owned(), stamped, Durability::Volatile)?;
        Ok(())
    }
}

impl Source for Reading<'_> {
    fn entries(&mut self, rel: &Path, stat: Stat) -> Entries {
        let at = key(rel);
        let read = self
            .stat(&at, stat)
            .and_then(|()| Ok(self.graph.query(&ENTRIES, &at)?));
        self.dirs.insert(at);
        match read {
            Ok(entries) => entries.as_ref().clone(),
            Err(e) => Entries {
                raw: Vec::new(),
                errors: vec![e.to_string()],
            },
        }
    }

    fn text(&mut self, rel: &Path) -> Option<String> {
        let at = key(rel);
        let stat = Stat::of(&fs::symlink_metadata(self.root.join(rel)).ok()?);
        self.stat(&at, stat).ok()?;
        self.texts.insert(at.clone());
        let content = self.graph.query(&CONTENT, &at).ok()?;
        content.text.clone()
    }
}

/// The `stat` input of a file or directory the project holds.
pub(super) fn stamp(stat: Stat, started: i64) -> Stamped {
    Some((stat, stat.racy(started).then_some(started)))
}

/// Sets the root and the ignore configuration.
pub(super) fn set_root(graph: &mut Graph, root: &Path, ignores: &[String]) -> Result<(), Error> {
    graph.set_input(&ROOT, &(), key(root), Durability::Static)?;
    let ignores = blake3::hash(ignores.join("\n").as_bytes())
        .to_hex()
        .to_string();
    graph.set_input(&CONFIG, &(), ignores, Durability::Stable)?;
    Ok(())
}

/// Sets the `stat` of each file of the walk, and takes it from what this
/// process set before and the project no longer holds: `before` are those
/// paths, `fresh` the directories and ignore files the walk set one for and
/// `walk` the files. Returns the paths that hold one now.
pub(super) fn set_stats(
    graph: &mut Graph,
    started: i64,
    before: &BTreeSet<String>,
    reading: (BTreeSet<String>, BTreeSet<String>),
    stats: &std::collections::BTreeMap<PathBuf, Stat>,
) -> Result<BTreeSet<String>, Error> {
    let (dirs, texts) = reading;
    let mut held = dirs;
    for (rel, stat) in stats {
        let at = key(rel);
        graph.set_input(&STAT, &at, stamp(*stat, started), Durability::Volatile)?;
        held.insert(at);
    }
    for gone in before.iter().chain(&texts).filter(|at| !held.contains(*at)) {
        graph.set_input(&STAT, gone, None, Durability::Volatile)?;
    }
    Ok(held)
}

/// Sets the overlays, and unsets those of the last run that are gone: `before`
/// are the paths this process set at the last run. Returns the paths set now.
pub(super) fn set_overlays(
    graph: &mut Graph,
    before: &BTreeSet<String>,
    overlays: &Overlays,
) -> Result<BTreeSet<String>, Error> {
    let overlaid: BTreeSet<String> = overlays.keys().map(|p| key(p)).collect();
    for (path, text) in overlays {
        let content = Some(Content::of_text(text));
        graph.set_input(&OVERLAY, &key(path), content, Durability::Volatile)?;
    }
    for gone in before.iter().filter(|p| !overlaid.contains(*p)) {
        graph.set_input(&OVERLAY, gone, None, Durability::Volatile)?;
    }
    // Reading an overlay registers its kind, which a stored `content` that
    // read none depends on, whether or not one is set.
    graph.input(&OVERLAY, &String::new())?;
    Ok(overlaid)
}
