//! How the regions a host file routed to other providers are told apart from
//! the file: which languages it holds, and which of them are example code.
//! The providers that analyze the regions know none of it; the engine states
//! it on the files and modules they return.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use lighthouse_plugin::EmbeddedRegion;

/// The reserved context key of a injection that marks its regions as
/// example code.
const EXAMPLE: &str = "example";

/// The regions of each host file, by the language they were routed to, in the
/// order the providers were asked: whether every one of them is example code.
#[derive(Default)]
pub(super) struct Views {
    by_path: BTreeMap<PathBuf, Vec<(String, bool)>>,
}

impl Views {
    /// Notes that `path` has `sources` routed to the provider of `language`.
    pub(super) fn add(&mut self, path: &Path, language: &str, sources: &[EmbeddedRegion]) {
        let example = sources.iter().all(is_example);
        self.by_path
            .entry(path.to_owned())
            .or_default()
            .push((language.to_owned(), example));
    }

    /// The languages the regions of `path` were routed to.
    pub(super) fn of(&self, path: &Path) -> Vec<(String, bool)> {
        self.by_path.get(path).cloned().unwrap_or_default()
    }
}

/// Whether a region is example code, as its injection says.
pub(super) fn is_example(source: &EmbeddedRegion) -> bool {
    source
        .context
        .get(EXAMPLE)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
