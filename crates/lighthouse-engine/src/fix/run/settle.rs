//! The safe fixes that follow a change. Whoever makes a fix, a rule, a command
//! or an agent, the code it leaves can break another decision whose fix is
//! mechanical: an agent places code where it reads best, a reorder moves lines
//! a formatter would break, an order rule is fixed by a reorder and not by a
//! rewrite. So a change is judged on itself plus the safe fixes of the
//! findings it introduced in its files, applied in the order of their
//! priority, in at most [`PASSES`] passes. Those fixes become part of the fix:
//! its edits say the text each of its files ends with, so that a preview shows
//! what the run would write. A finding that no safe fix clears still counts
//! against the change. The scope of the fix is checked on the program's own
//! diff, before this, so these edits are not held to it.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use lighthouse_model::{Fingerprint, Safety};

use super::{
    super::FixedBy,
    super::edits::{TextEdit, apply, replace_changed},
    super::propose::{Phase, Proposal, Taking},
    Orchestrator,
    rollback::{kept_proposals, worse},
};
use crate::{Error, Overlays, Run};

/// How many times the safe fixes of the findings a change introduced are
/// applied: the findings those fixes introduce get one more.
const PASSES: usize = 2;

/// Makes the edits of the fixes among `kept` and `merged` say the text each
/// of their files ends with in `texts`, when the safe fixes that followed the
/// change changed it further, so that a preview shows what the run would
/// write. `bases` holds the texts the round started from. A file that several
/// fixes edit keeps their edits: the follow-ups cannot be told apart there.
pub(super) fn fold(
    kept: &mut [Proposal],
    merged: &mut [(Proposal, Fingerprint)],
    texts: &Overlays,
    bases: &BTreeMap<PathBuf, String>,
) {
    for index in 0..kept.len() {
        let fingerprint = kept[index].fingerprint.clone();
        let files = kept[index].files.clone();
        for file in files {
            let alone = kept.iter().filter(|p| p.files.contains(&file)).count() == 1;
            let (Some(base), Some(end)) = (bases.get(&file), texts.get(&file)) else {
                continue;
            };
            if !alone || stands(&kept[index], &file, base, end) {
                continue;
            }
            let model = kept[index].fixer == FixedBy::Model;
            let edit = restated(&file, base, end, model);
            let sharing = merged
                .iter_mut()
                .filter(|(_, into)| *into == fingerprint)
                .map(|(p, _)| p);
            for p in std::iter::once(&mut kept[index]).chain(sharing) {
                p.edits.retain(|e| e.file != file);
                p.edits.extend(edit.clone());
            }
        }
    }
}

/// Whether the edits of `proposal` in `file` make `end` of `base`.
fn stands(proposal: &Proposal, file: &Path, base: &str, end: &str) -> bool {
    let mine: Vec<&TextEdit> = proposal.edits.iter().filter(|e| e.file == file).collect();
    apply(base, &mine).is_ok_and(|made| made == end)
}

/// The edit that turns `base` into `end`: the whole file for a fix by a
/// model, which is made of whole files, else the lines that differ.
fn restated(file: &Path, base: &str, end: &str, whole: bool) -> Option<TextEdit> {
    if whole {
        return Some(TextEdit {
            file: file.to_owned(),
            range: 0..base.len(),
            text: end.to_owned(),
        });
    }
    replace_changed(file, base, end)
}

impl Orchestrator<'_> {
    /// Applies the safe fixes of the findings that the change of `changed`
    /// introduced (those of `after` that `before` did not have), in the order
    /// of their priority, pass after pass; the check of the resulting state.
    /// A pass that makes a file worse, or returns to a state seen, is undone
    /// and ends it. A run that names its own fixer settles nothing.
    pub(super) fn settle(
        &mut self,
        before: &Run,
        mut after: Run,
        changed: &[PathBuf],
    ) -> Result<Run, Error> {
        if self.proposer.run.fixer.is_some() || changed.is_empty() {
            return Ok(after);
        }
        let known: BTreeSet<&Fingerprint> =
            before.diagnostics.iter().map(|d| &d.fingerprint).collect();
        let mut seen = BTreeSet::from([self.state_hash()]);
        for _ in 0..PASSES {
            let introduced: BTreeSet<Fingerprint> = after
                .diagnostics
                .iter()
                .filter(|d| changed.contains(&d.file) && !known.contains(&d.fingerprint))
                .map(|d| d.fingerprint.clone())
                .collect();
            if introduced.is_empty() {
                break;
            }
            let taking = Taking::Introduced {
                files: changed,
                findings: &introduced,
            };
            let (found, _) = self
                .proposer
                .propose(&after, &self.texts, Phase::Rules, taking)?;
            let mut safe: Vec<Proposal> = found
                .into_iter()
                .filter(|p| p.safety == Safety::Safe && p.files.iter().all(|f| changed.contains(f)))
                .collect();
            safe.sort_by_key(|p| Reverse(p.priority));
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
