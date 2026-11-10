//! What a walk reads through the graph, and the inputs it sets for it.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use lighthouse_graph::{Durability, Error, Graph};

use crate::{CONTENT, ENTRIES, Entries, Held, STAT, Stat, key};

/// What a walk is given by the graph: a directory is a `stat` input and the
/// `entries` query on it, the text of an ignore file is its `content`. It
/// keeps the paths it set a `stat` for.
pub(super) struct Reading<'a> {
    graph: &'a mut Graph,
    held: &'a mut Held,
    root: PathBuf,
    started: i64,
    dirs: BTreeSet<String>,
    texts: BTreeSet<String>,
}

impl<'a> Reading<'a> {
    pub(super) fn new(graph: &'a mut Graph, held: &'a mut Held, root: &Path, started: i64) -> Self {
        Self {
            graph,
            held,
            root: root.to_owned(),
            started,
            dirs: BTreeSet::new(),
            texts: BTreeSet::new(),
        }
    }

    /// What the directory at the project-relative `rel`, whose `lstat` is
    /// `stat`, holds.
    pub(super) fn entries(&mut self, rel: &Path, stat: Stat) -> Entries {
        let at = key(rel);
        let read = self
            .stat(&at, stat)
            .and_then(|()| self.graph.query(&ENTRIES, &at));
        self.dirs.insert(at);
        match read {
            Ok(entries) => entries.as_ref().clone(),
            Err(e) => Entries {
                raw: Vec::new(),
                errors: vec![e.to_string()],
            },
        }
    }

    /// Whether an overlay stands in for the file at the project-relative
    /// `rel`.
    pub(super) fn overlaid(&self, rel: &Path) -> bool {
        self.held.overlaid().contains(&key(rel))
    }

    /// The text of the ignore file at the project-relative `rel`, if it can be
    /// read as text.
    pub(super) fn text(&mut self, rel: &Path) -> Option<String> {
        let at = key(rel);
        let stat = Stat::of(&fs::symlink_metadata(self.root.join(rel)).ok()?);
        self.stat(&at, stat).ok()?;
        self.texts.insert(at.clone());
        let content = self.graph.query(&CONTENT, &at).ok()?;
        content.text.clone()
    }

    fn stat(&mut self, at: &str, stat: Stat) -> Result<(), Error> {
        let stamped = stat.stamped(self.started);
        self.graph
            .set_input(&STAT, &at.to_owned(), stamped, Durability::Volatile)
    }

    /// Sets the `stat` of each file of the walk, and takes it from what this
    /// process set before and the project no longer holds: the paths of the
    /// last walk, and the ignore files this one read.
    pub(super) fn settle(self, stats: &BTreeMap<PathBuf, Stat>) -> Result<(), Error> {
        let mut held = self.dirs;
        for (rel, stat) in stats {
            let at = key(rel);
            self.graph
                .set_input(&STAT, &at, stat.stamped(self.started), Durability::Volatile)?;
            held.insert(at);
        }
        for gone in self
            .held
            .statted
            .iter()
            .chain(&self.texts)
            .filter(|at| !held.contains(*at))
        {
            self.graph
                .set_input(&STAT, gone, None, Durability::Volatile)?;
        }
        self.held.statted = held;
        Ok(())
    }
}
