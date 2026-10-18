//! The safe fixes of the rules, applied to what an agent left before it is
//! judged. An agent places code where it reads best, and an order rule such as
//! `design/declaration-groups` is fixed by a reorder, not by a rewrite: the
//! verdict is on the findings that remain after the project's own tools have
//! had their say. The scope of the fix is checked on the agent's own diff,
//! before this, so these edits are not held to it.

use std::{collections::BTreeSet, path::PathBuf};

use lighthouse_model::Safety;

use super::{
    super::propose::{Phase, Proposal},
    super::{FixedBy, MAX_ROUNDS},
    Orchestrator,
    rollback::{kept_proposals, worse},
};
use crate::{Error, Overlays, Run};

/// Whether `kept` holds a fix by a model.
pub(super) fn has_model(kept: &[Proposal]) -> bool {
    kept.iter().any(|p| p.fixer == FixedBy::Model)
}

/// Makes the whole-file edits of the fixes by a model say the text each of
/// their files ends with in `texts`, so that a preview shows what the run
/// would write.
pub(super) fn restate(
    kept: &mut [Proposal],
    merged: &mut [(Proposal, lighthouse_model::Fingerprint)],
    texts: &Overlays,
) {
    let models = kept
        .iter_mut()
        .chain(merged.iter_mut().map(|(p, _)| p))
        .filter(|p| p.fixer == FixedBy::Model);
    for edit in models.flat_map(|p| p.edits.iter_mut()) {
        if let Some(text) = texts.get(&edit.file) {
            edit.text.clone_from(text);
        }
    }
}

impl Orchestrator<'_> {
    /// Applies the safe fixes the rules propose for the findings in `changed`
    /// to the texts, round after round until none applies; the check of the
    /// resulting state. A round that makes a file worse, or returns to a state
    /// seen, is undone and ends it. A run that names its own fixer settles
    /// nothing.
    pub(super) fn settle(&mut self, mut after: Run, changed: &[PathBuf]) -> Result<Run, Error> {
        if self.proposer.run.fixer.is_some() || changed.is_empty() {
            return Ok(after);
        }
        let mut seen = BTreeSet::from([self.state_hash()]);
        for _ in 0..MAX_ROUNDS {
            let (found, _) = self
                .proposer
                .propose(&after, &self.texts, Phase::Rules, changed)?;
            let safe = found
                .into_iter()
                .filter(|p| p.safety == Safety::Safe && p.files.iter().all(|f| changed.contains(f)))
                .collect();
            let (kept, _, _) = kept_proposals(safe);
            let round = self.round(&kept, &after);
            if round.texts.is_empty() {
                break;
            }
            let prior: Vec<(PathBuf, Option<String>)> = round
                .texts
                .into_iter()
                .map(|(file, text)| (file.clone(), self.texts.insert(file, text)))
                .collect();
            let next = self.proposer.engine.check_overlaid(&[], &[], &self.texts)?;
            if seen.insert(self.state_hash()) && worse(&after, &next, changed).is_empty() {
                after = next;
                continue;
            }
            for (file, text) in prior {
                match text {
                    Some(text) => self.texts.insert(file, text),
                    None => self.texts.remove(&file),
                };
            }
            break;
        }
        Ok(after)
    }
}
