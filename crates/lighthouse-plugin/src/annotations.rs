use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use serde_json::Value;

/// What the suppression directives (`lighthouse-disable ...`) of each file say
/// once every other rule has run: for each rule a directive names, the rule,
/// the reason, and whether it suppressed a finding. The engine puts them in
/// the run's [`Memo`](crate::Memo) before it runs the rules that read them
/// (a rule for which [`Rule::after_suppression`](crate::Rule::after_suppression)
/// is true).
#[derive(Default)]
pub struct Annotations {
    files: Mutex<BTreeMap<PathBuf, Vec<Value>>>,
}

impl Annotations {
    /// Keeps `annotations` as those of `file`.
    pub fn insert(&self, file: PathBuf, annotations: Vec<Value>) {
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(file, annotations);
    }

    /// The annotations of `file`, empty for a file with no directive; `None`
    /// before the engine has judged the directives, which a rule reads as an
    /// error, never as "none".
    pub fn of(&self, file: &Path) -> Option<Vec<Value>> {
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(file)
            .cloned()
    }
}
