//! Whom a finding that is new in a file a round did not change is charged to.
//! A fix is judged by what it leaves in the project, but not everything that
//! turns up in the project is its doing: a rule that reads one file alone
//! (`local`) cannot have been moved by a change elsewhere, one that reads the
//! symbols around its subject (`neighbors`) by a change in the files that hold
//! them, and only a rule that reads the whole project (`global`, or one that
//! declares nothing) by any change of the round.

use std::{
    cell::OnceCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::super::model::reach;
use crate::Outcome;

/// The charges for what the check `after` found more of than the check
/// `before`, when the files `changed` were changed between them.
pub(super) struct Blame<'a> {
    before: &'a Outcome,
    after: &'a Outcome,
    changed: &'a [PathBuf],
    /// What each changed file reaches, found when a rule needs it.
    reached: OnceCell<Vec<BTreeSet<PathBuf>>>,
}

impl<'a> Blame<'a> {
    pub(super) fn new((before, after): (&'a Outcome, &'a Outcome), changed: &'a [PathBuf]) -> Self {
        Self {
            before,
            after,
            changed,
            reached: OnceCell::new(),
        }
    }

    pub(super) fn runs(&self) -> (&'a Outcome, &'a Outcome) {
        (self.before, self.after)
    }

    pub(super) fn changed(&self) -> &'a [PathBuf] {
        self.changed
    }

    /// The files, and for each the rules, with more findings of error or warn
    /// severity after than before, wherever in the file they are.
    pub(super) fn grown(&self) -> Vec<(&'a Path, &'a str)> {
        type Key<'k> = (&'k Path, &'k str);
        fn count(run: &Outcome) -> BTreeMap<Key<'_>, usize> {
            let mut counts = BTreeMap::new();
            for d in run.loud() {
                *counts
                    .entry((d.file.as_path(), d.rule_id.as_str()))
                    .or_insert(0) += 1;
            }
            counts
        }
        let known = count(self.before);
        count(self.after)
            .into_iter()
            .filter(|(key, now)| known.get(key).copied().unwrap_or(0) < *now)
            .map(|(key, _)| key)
            .collect()
    }

    /// The changed files a new finding of `rule` in the unchanged `file` is
    /// charged to: all of them, until the findings say what they read.
    pub(super) fn charged(&self, _file: &Path, _rule: &str) -> Vec<&'a PathBuf> {
        self.changed.iter().collect()
    }

    /// The changed files that have an edge to or from `file`, in either state.
    pub(super) fn reaching(&self, file: &Path) -> Vec<&'a PathBuf> {
        self.changed
            .iter()
            .zip(self.reached())
            .filter(|(_, reached)| reached.contains(file))
            .map(|(changed, _)| changed)
            .collect()
    }

    /// The unchanged files with a new finding that is charged to some changed
    /// files, with those files: the dependents of the change.
    pub(super) fn dependents(&self) -> BTreeMap<PathBuf, BTreeSet<PathBuf>> {
        let mut found: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
        for (file, rule) in self.grown() {
            if self.changed.iter().any(|c| c == file) {
                continue;
            }
            let causes = self.charged(file, rule);
            if !causes.is_empty() {
                found
                    .entry(file.to_path_buf())
                    .or_default()
                    .extend(causes.into_iter().cloned());
            }
        }
        found
    }

    fn reached(&self) -> &[BTreeSet<PathBuf>] {
        self.reached.get_or_init(|| {
            self.changed
                .iter()
                .map(|file| {
                    reach(std::slice::from_ref(file), self.before, self.after)
                        .into_iter()
                        .collect()
                })
                .collect()
        })
    }
}
