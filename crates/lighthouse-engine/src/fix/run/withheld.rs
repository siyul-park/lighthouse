//! The verification of the fixes the run may not write. They are checked over
//! the state the rounds left, in groups whose edits do not overlap, each group
//! by one check of the project; a group that is not accepted whole is halved,
//! down to one fix, which is judged by itself. A fix is applied later by itself,
//! so a group is accepted whole only when that cannot differ from its fixes
//! taken one at a time: none of its files is edited by two of its fixes, or
//! all of its fixes stand applied with no safe follow-up reported.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::Fingerprint;

use super::{
    super::propose::Proposal,
    super::{DeclinedFix, PreviewedFix},
    Orchestrator,
    chain::Mark,
    rollback::kept_proposals,
};
use crate::{Error, Run};

/// The state of a run before a group is tried, to put back what is not kept.
struct Trial {
    mark: Mark,
    declined: BTreeMap<Fingerprint, DeclinedFix>,
    applied: usize,
    notes: usize,
}

impl Orchestrator<'_> {
    /// Verifies the `withheld` fixes the run may not write, over the state the
    /// rounds left, and reports those the checks accept as previews. The texts
    /// are left as they were.
    pub(super) fn preview(&mut self, outcome: &Run, withheld: Vec<Proposal>) -> Result<(), Error> {
        let order = listing(&withheld);
        let mut shown = Vec::new();
        for group in layers(withheld) {
            self.preview_group(outcome, group, &mut shown)?;
        }
        // The previews are listed in the order of the fixes taken one file at
        // a time, however the groups were verified.
        shown.sort_by_key(|(fingerprint, _)| order.iter().position(|f| f == fingerprint));
        self.previews
            .extend(shown.into_iter().map(|(_, preview)| preview));
        Ok(())
    }

    fn preview_group(
        &mut self,
        outcome: &Run,
        group: Vec<Proposal>,
        shown: &mut Vec<(Fingerprint, PreviewedFix)>,
    ) -> Result<(), Error> {
        let trial = self.begin();
        let wanted: BTreeSet<Fingerprint> = group.iter().map(|p| p.fingerprint.clone()).collect();
        let alone = group.len() == 1 || files_unshared(&group);
        self.apply_round(outcome, group.clone(), self.rounds + 1)?;
        if alone || self.standing_whole(&trial, &wanted) {
            self.withdraw_previews(trial, shown);
            return Ok(());
        }
        self.rewind(trial, &wanted);
        let mut first = group;
        let second = first.split_off(first.len() / 2);
        self.preview_group(outcome, first, shown)?;
        self.preview_group(outcome, second, shown)
    }

    /// Withdraws the fixes applied since `trial` to previews, kept in `shown`
    /// with the fingerprint of their fix.
    fn withdraw_previews(&mut self, trial: Trial, shown: &mut Vec<(Fingerprint, PreviewedFix)>) {
        let fingerprints: Vec<Fingerprint> = self.applied[trial.applied..]
            .iter()
            .filter(|a| !a.follow_up)
            .filter(|a| {
                self.previewable
                    .get(&a.fingerprint)
                    .is_some_and(|standing| standing.shown.is_ok())
            })
            .map(|a| a.fingerprint.clone())
            .collect();
        let before = self.previews.len();
        self.withdraw(trial.mark);
        shown.extend(
            fingerprints
                .into_iter()
                .zip(self.previews.split_off(before)),
        );
    }

    fn begin(&self) -> Trial {
        Trial {
            mark: self.mark(),
            declined: self.declined.clone(),
            applied: self.applied.len(),
            notes: self.notes.len(),
        }
    }

    /// Whether every fix of `wanted` stands applied since `trial`, none of the
    /// reported fixes being a follow-up.
    fn standing_whole(&self, trial: &Trial, wanted: &BTreeSet<Fingerprint>) -> bool {
        let since = &self.applied[trial.applied..];
        since.iter().all(|a| !a.follow_up)
            && since.len() == wanted.len()
            && since.iter().all(|a| wanted.contains(&a.fingerprint))
    }

    /// Puts the run back as it was at `trial`, as if the group was not tried.
    fn rewind(&mut self, trial: Trial, wanted: &BTreeSet<Fingerprint>) {
        (self.texts, self.original, self.rounds) =
            (trial.mark.texts, trial.mark.original, trial.mark.rounds);
        self.applied.truncate(trial.applied);
        self.notes.truncate(trial.notes);
        self.declined = trial.declined;
        self.previewable
            .retain(|fingerprint, _| !wanted.contains(fingerprint));
    }
}

/// The fixes of `withheld` as they were verified one batch after another, in
/// batches in which none edits a file another of the batch does.
fn listing(withheld: &[Proposal]) -> Vec<Fingerprint> {
    let mut batches: Vec<Vec<Proposal>> = Vec::new();
    for p in withheld {
        let free = batches.iter().position(|batch| {
            !batch
                .iter()
                .any(|q| q.files.iter().any(|f| p.files.contains(f)))
        });
        match free {
            Some(at) => batches[at].push(p.clone()),
            None => batches.push(vec![p.clone()]),
        }
    }
    let mut order = Vec::new();
    for mut batch in batches {
        batch.sort_by_key(|p| std::cmp::Reverse(p.priority));
        let (kept, merged, _) = kept_proposals(batch);
        for k in kept {
            order.push(k.fingerprint.clone());
            order.extend(
                merged
                    .iter()
                    .filter(|(_, into)| *into == k.fingerprint)
                    .map(|(p, _)| p.fingerprint.clone()),
            );
        }
    }
    order
}

/// `proposals` in order, in groups in which no edit overlaps another of the
/// group; a proposal that repeats the edits of another is in its group.
fn layers(mut proposals: Vec<Proposal>) -> Vec<Vec<Proposal>> {
    proposals.sort_by_key(|p| std::cmp::Reverse(p.priority));
    let mut groups = Vec::new();
    while !proposals.is_empty() {
        let (kept, merged, dropped) = kept_proposals(proposals);
        let mut group = kept;
        group.extend(merged.into_iter().map(|(p, _)| p));
        groups.push(group);
        proposals = dropped;
    }
    groups
}

/// Whether no file is edited by two of `group`.
fn files_unshared(group: &[Proposal]) -> bool {
    let mut seen = BTreeSet::new();
    group.iter().all(|p| p.files.iter().all(|f| seen.insert(f)))
}

#[cfg(test)]
mod tests;
