//! What reading one SARIF run left out or could not do, and the files it
//! read: said once per run as notices, never silently.

use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use lighthouse_plugin::{Context, Notices};

/// The tally of one run of a log.
#[derive(Default)]
pub(super) struct Tally {
    /// Files of results the project does not have, in the order met.
    outside: Vec<PathBuf>,
    /// Files whose text could not be read, so their columns were not converted.
    unreadable: Vec<PathBuf>,
    texts: BTreeMap<PathBuf, Option<String>>,
}

impl Tally {
    /// A result lies in a file the project does not have.
    pub(super) fn left_out(&mut self, path: &Path) {
        self.outside.push(path.to_owned());
    }

    /// The text of a project file, read once; `None` when it cannot be read.
    pub(super) fn text(&mut self, ctx: &Context, path: &Path) -> Option<&str> {
        if !self.texts.contains_key(path) {
            let text = ctx.workspace.read(path).ok().map(Cow::into_owned);
            if text.is_none() {
                self.unreadable.push(path.to_owned());
            }
            self.texts.insert(path.to_owned(), text);
        }
        self.texts.get(path)?.as_deref()
    }

    /// Says what happened in the run of `tool`.
    pub(super) fn tell(&self, notices: &Notices, tool: &str) {
        if let Some(first) = self.outside.first() {
            notices.push(format!(
                "{tool}: {} result(s) in files the project does not have were dropped, the first in {}",
                self.outside.len(),
                first.display()
            ));
        }
        if let Some(first) = self.unreadable.first() {
            notices.push(format!(
                "{tool}: {} file(s) could not be read to convert columns, so their results start at column 1, the first {}",
                self.unreadable.len(),
                first.display()
            ));
        }
    }
}
