//! What became of the findings given to a model, per decision.

use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
};

/// Why a fix by a model was declined, as a class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::fix) enum Class {
    /// There was no code to give the model.
    NoExtent,
    /// No model answered: the budget was spent or they failed.
    Unanswered,
    /// The agent changed what it was not asked to.
    OutOfScope,
    /// The answer was not a replacement.
    Unparsed,
    /// The answer left the code as it was.
    Unchanged,
    /// The finding was still there.
    Unresolved,
    /// A finding of error or warn severity that was not there before.
    Introduced,
    /// The analysis of the file was incomplete.
    Incomplete,
    /// The file was rolled back for another reason, or with another fix.
    RolledBack,
}

impl Class {
    pub(in crate::fix) fn name(self) -> &'static str {
        match self {
            Self::NoExtent => "no-extent",
            Self::Unanswered => "unanswered",
            Self::OutOfScope => "out-of-scope",
            Self::Unparsed => "unparsed",
            Self::Unchanged => "unchanged",
            Self::Unresolved => "unresolved",
            Self::Introduced => "introduced",
            Self::Incomplete => "incomplete",
            Self::RolledBack => "rolled-back",
        }
    }
}

/// A step in the life of a fix by a model.
#[derive(Debug, Clone, Copy)]
pub(in crate::fix) enum Event {
    /// The model was asked.
    Asked,
    /// The model replied.
    Answered,
    /// The checks accepted the fix.
    Verified,
    /// The fix is written (or would be, in a dry run).
    Applied,
    /// The fix is left as a preview.
    Previewed,
    Declined(Class),
}

/// What happened to the findings of one decision given to a model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelCounts {
    /// Findings the model was asked about.
    pub asked: usize,
    /// Of them, those it replied to.
    pub answered: usize,
    /// Replies the checks accepted: the finding gone, nothing new in the file.
    pub verified: usize,
    /// Verified fixes that were written.
    pub applied: usize,
    /// Verified fixes left as previews.
    pub previewed: usize,
    /// Findings left as they were, by the class of the reason.
    pub declined: BTreeMap<String, usize>,
}

/// The counts of a run, by decision. Fixers are asked from many threads.
#[derive(Debug, Default)]
pub(in crate::fix) struct Account(Mutex<BTreeMap<String, ModelCounts>>);

impl Account {
    pub(in crate::fix) fn add(&self, decision: &str, event: Event) {
        let mut all = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let counts = all.entry(decision.to_owned()).or_default();
        match event {
            Event::Asked => counts.asked += 1,
            Event::Answered => counts.answered += 1,
            Event::Verified => counts.verified += 1,
            Event::Applied => counts.applied += 1,
            Event::Previewed => counts.previewed += 1,
            Event::Declined(class) => {
                *counts.declined.entry(class.name().to_owned()).or_default() += 1;
            }
        }
    }

    /// The counts, by decision.
    pub(in crate::fix) fn counts(&self) -> BTreeMap<String, ModelCounts> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
