//! The safe fixes that follow a change. Whoever makes a fix, a rule, a command
//! or an agent, the code it leaves can break another decision whose fix is
//! mechanical: an agent places code where it reads best, a reorder moves lines
//! a formatter would break, an order rule is fixed by a reorder and not by a
//! rewrite. So a change is judged on itself plus the safe fixes of the
//! findings it introduced in its files, applied in the order of their
//! priority, in at most [`PASSES`] passes. Those fixes become part of the fix:
//! its edits say the text each of its files ends with, so that a preview shows
//! what the run would write. In a file that several fixes edited they cannot
//! be told apart, so they are reported as fixes of their own, each of its
//! decision. A finding that no safe fix clears still counts against the
//! change. The scope of the fix is checked on the program's own
//! diff, before this, so these edits are not held to it.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use lighthouse_model::{Fingerprint, Safety};

use super::{
    super::edits::{TextEdit, apply, replace_changed},
    super::propose::{Phase, Proposal, Taking},
    super::{AppliedFix, FixedBy},
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
/// fixes edit keeps their edits: the follow-ups cannot be told apart there,
/// and [`Orchestrator::report_follow_ups`] reports them.
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
            let alone = editors(kept, &file) == 1;
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

/// How many of `kept` edit `file`.
fn editors(kept: &[Proposal], file: &Path) -> usize {
    kept.iter()
        .filter(|p| p.files.iter().any(|f| f == file))
        .count()
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
    /// and ends it. A run that names its own fixer settles nothing. The fixes
    /// that stay applied come with the files each changed.
    pub(super) fn settle(
        &mut self,
        before: &Run,
        mut after: Run,
        changed: &[PathBuf],
    ) -> Result<(Run, Vec<Proposal>), Error> {
        let mut followed = Vec::new();
        if self.proposer.run.fixer.is_some() || changed.is_empty() {
            return Ok((after, followed));
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
            let (kept, merged, _) = kept_proposals(safe);
            let round = self.round(&kept, &after);
            if round.texts.is_empty() {
                break;
            }
            let made: Vec<Proposal> = kept
                .iter()
                .chain(merged.iter().map(|(p, _)| p))
                .map(|p| Proposal {
                    files: p
                        .files
                        .iter()
                        .filter(|f| round.texts.contains_key(*f))
                        .cloned()
                        .collect(),
                    ..p.clone()
                })
                .filter(|p| !p.files.is_empty())
                .collect();
            let prior: Vec<(PathBuf, Option<String>)> = round
                .texts
                .into_iter()
                .map(|(file, text)| (file.clone(), self.texts.insert(file, text)))
                .collect();
            let next = self.proposer.engine.check_overlaid(&[], &[], &self.texts)?;
            if seen.insert(self.state_hash()) && worse(&after, &next, changed).is_empty() {
                after = next;
                followed.extend(made);
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
        Ok((after, followed))
    }

    /// Reports the safe fixes `followed` that stay applied in the files that
    /// several of the `kept` fixes of the round edit, and are not rolled back:
    /// what they wrote there belongs to no one of those fixes, so each is an
    /// applied fix of its own decision, marked as a follow-up.
    pub(super) fn report_follow_ups(
        &mut self,
        followed: Vec<Proposal>,
        kept: &[Proposal],
        rolled: &BTreeMap<PathBuf, String>,
        round: usize,
    ) {
        for p in followed {
            let files: Vec<PathBuf> = p
                .files
                .into_iter()
                .filter(|f| editors(kept, f) > 1 && !rolled.contains_key(f))
                .collect();
            if files.is_empty() {
                continue;
            }
            self.applied.push(AppliedFix {
                fingerprint: p.fingerprint,
                rule_id: p.rule_id,
                fixer: p.fixer,
                description: p.description,
                safety: p.safety,
                files,
                round,
                preview: None,
                follow_up: true,
            });
        }
    }
}
