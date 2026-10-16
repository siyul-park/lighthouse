//! Reading the decision log without the cache: the judgments that are labels,
//! and the revisions that bind models to decisions.

use std::{collections::BTreeMap, path::Path};

use crate::{Error, JudgmentEvent, RevisionEvent, RevisionSpec, Store, log};

/// What the decision log of a project holds.
#[derive(Debug, Default)]
pub struct Records {
    /// Every judgment, with the suppressions that go with it.
    pub judgments: Vec<JudgmentEvent>,
    pub revisions: Vec<RevisionEvent>,
    /// What the reader skipped.
    pub notices: Vec<String>,
}

impl Records {
    /// The records of the project at `root`; none when it has no log.
    pub fn read(root: &Path) -> Result<Self, Error> {
        log::read(&Store::log_path_in(root))
    }

    /// The latest revision of `decision` (its uid, else its name), in log order.
    pub fn latest(&self, decision: &str) -> Option<&RevisionEvent> {
        self.revisions
            .iter()
            .rev()
            .find(|revision| revision.spec.decision == decision)
    }

    /// The model each decision is bound to by its latest revision, by the
    /// decision's uid (its name when it has none). A revision that binds no
    /// model unbinds it.
    pub fn attached(&self) -> BTreeMap<String, String> {
        let mut bound = BTreeMap::new();
        for revision in &self.revisions {
            let spec = &revision.spec;
            match &spec.model {
                Some(model) => bound.insert(spec.decision.clone(), model.clone()),
                None => bound.remove(&spec.decision),
            };
        }
        bound
    }
}

/// Appends a revision to the log of the project at `root` and returns its id.
pub fn append_revision(root: &Path, spec: RevisionSpec) -> Result<String, Error> {
    let id = log::id_of(&spec)?;
    let line = log::line(&id, spec)?;
    log::append(&Store::log_path_in(root), &[line])?;
    Ok(id)
}
