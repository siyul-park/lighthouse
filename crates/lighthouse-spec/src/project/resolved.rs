use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex, PoisonError},
};

use super::Rules;

/// The projects a file was resolved against and the overrides it matched.
type Key = (u64, Vec<bool>);

/// What a project resolved to, by the set of overrides a file matched: files
/// that match the same ones share one merge. A clone starts with nothing.
#[derive(Default)]
pub(super) struct Resolved(Mutex<HashMap<Key, Arc<Rules>>>);

impl Resolved {
    /// The rules for the overrides `matched` flags, of the projects `projects`
    /// identifies: kept, else `merge`d from the flags and kept.
    pub(super) fn get_or_merge(
        &self,
        projects: u64,
        matched: Vec<bool>,
        merge: impl FnOnce(&[bool]) -> Rules,
    ) -> Arc<Rules> {
        let key = (projects, matched);
        let lock = || self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = lock().get(&key) {
            return Arc::clone(found);
        }
        let merged = Arc::new(merge(&key.1));
        Arc::clone(lock().entry(key).or_insert(merged))
    }
}

impl Clone for Resolved {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl fmt::Debug for Resolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Resolved")
    }
}
