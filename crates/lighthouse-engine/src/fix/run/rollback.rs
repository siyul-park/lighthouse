//! A round's verdict: which proposals apply together, which got a file worse
//! and are rolled back whole, and what became of each.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use lighthouse_model::Fingerprint;

use lighthouse_protocol::hash;

use super::{
    super::edits::{TextEdit, keeps_declarations, overlap},
    super::model::{Class, Event, Verdict, judge},
    super::preview::preview_of,
    super::propose::Proposal,
    super::{AppliedFix, DeclinedFix, FixedBy},
    Orchestrator, Standing,
    blame::Blame,
    displaced::{displaced, moved_files},
    settle::{Joined, fold},
};
use crate::{Error, Run};

impl Orchestrator<'_> {
    /// Applies what does not collide to the texts in memory, verifies them
    /// with a check over overlays, applies the safe fixes of what they
    /// introduced and drops what got worse. The check of the resulting state,
    /// when nothing was dropped.
    pub(super) fn apply_round(
        &mut self,
        before: &Run,
        proposals: Vec<Proposal>,
        round: usize,
    ) -> Result<Option<Run>, Error> {
        self.round = round;
        let started = Instant::now();
        let applied = self.apply_proposals(before, proposals, round);
        self.timings.apply += started.elapsed();
        applied
    }

    fn apply_proposals(
        &mut self,
        before: &Run,
        mut proposals: Vec<Proposal>,
        round: usize,
    ) -> Result<Option<Run>, Error> {
        // The higher priority wins a collision, and the order of the findings
        // breaks a tie (the sort is stable). Safety is no key: what the run
        // may not write was set apart before, and takes no span.
        proposals.sort_by_key(|p| Reverse(p.priority));
        let (kept, mut merged, dropped) = kept_proposals(proposals);
        self.decline_dropped(&dropped);
        let (mut kept, scattered) = self.keeping_declarations(kept);
        merged.retain(|(_, same)| kept.iter().any(|k| &k.fingerprint == same));
        for (p, why) in &scattered {
            self.decline_proposal(p, why.clone());
        }
        let mut candidates = self.round(&kept, before);
        let mut failed = std::mem::take(&mut candidates.failed);
        let changed: Vec<PathBuf> = candidates.texts.keys().cloned().collect();
        // The candidates stand in for the disk while they are checked; the
        // texts they replace are kept to put back what is rolled back.
        let mut prior: BTreeMap<PathBuf, Option<String>> = std::mem::take(&mut candidates.texts)
            .into_iter()
            .map(|(file, text)| (file.clone(), self.texts.insert(file, text)))
            .collect();
        let after = if failed.len() == kept.len() && changed.is_empty() {
            None
        } else {
            Some(self.recheck()?)
        };
        let (mut followed, mut joined) = (Vec::new(), Joined::default());
        // The index of the changed texts says what the moves did to their
        // files' symbols, before the safe fixes that follow touch them.
        let displaced = after.as_ref().map_or_else(BTreeMap::new, |run| {
            displaced(&moved_files(&kept), (&before.project, &run.project))
        });
        let after = match after {
            Some(run) => {
                let settled;
                let started = Instant::now();
                let outcome = self.settle(before, run, &changed, &kept);
                self.timings.settle += started.elapsed();
                (settled, followed, joined) = outcome?;
                fold(&mut kept, &mut merged, &self.texts, &candidates.bases);
                Some(settled)
            }
            None => None,
        };
        let mut verdict = Verdict::default();
        // The files whose fixes answer the change are verified with it.
        let scope: Vec<PathBuf> = changed.iter().chain(joined.prior.keys()).cloned().collect();
        if let Some(after) = &after {
            let blame = Blame::new(self.proposer.engine, (before, after), &scope);
            failed.extend(worse(&blame));
            for (file, why) in displaced {
                failed.entry(file).or_insert(why);
            }
            verdict = judge(&kept, &merged, before, after);
            failed.extend(verdict.files.clone());
        }
        let rolled = close_over(&kept, &joined.causes, failed);
        prior.extend(joined.prior.clone());
        for (file, reason) in &rolled {
            if let Some(text) = prior.remove(file) {
                self.restore_text(file, text);
            }
            self.note_rolled(file, reason, (&kept, &merged), &joined);
        }
        self.keep_previews(&kept, &merged, &rolled, &candidates.bases);
        candidates.bases.extend(joined.bases.clone());
        for file in scope.iter().filter(|f| !rolled.contains_key(*f)) {
            self.original.entry(file.clone()).or_insert_with(|| {
                let base = candidates.bases.remove(file).unwrap_or_default();
                let hash = hash::sha256(&base);
                (base, hash)
            });
        }
        if self.account(
            (&kept, &merged),
            &rolled,
            &verdict,
            &candidates.unchanged,
            round,
        ) {
            self.rounds = round;
        }
        self.report_follow_ups(followed, (&kept, &joined), &rolled, round);
        Ok(if rolled.is_empty() { after } else { None })
    }

    /// `kept` without the proposals that move declarations in a file whose
    /// edits of the round together would not keep its declarations, with why.
    fn keeping_declarations(
        &self,
        kept: Vec<Proposal>,
    ) -> (Vec<Proposal>, Vec<(Proposal, String)>) {
        let broken = scattering(&kept, |file| self.standing_text(file));
        let (gone, kept) = kept
            .into_iter()
            .partition::<Vec<_>, _>(|p| p.files.iter().any(|f| broken.contains_key(f)));
        let gone = gone
            .into_iter()
            .filter_map(|p| {
                let why = p.files.iter().find_map(|f| broken.get(f))?.clone();
                Some((p, why))
            })
            .collect();
        (kept, gone)
    }

    /// Says why `file` was rolled back and after the fixes of which rules: those
    /// that edit it, or the changed files it answers.
    fn note_rolled(
        &mut self,
        file: &Path,
        reason: &str,
        proposals: (&[Proposal], &[(Proposal, Fingerprint)]),
        joined: &Joined,
    ) {
        let by = joined.rules_for(file, proposals);
        let file = file.display();
        self.notes.push(format!(
            "rolled back {file}: {reason} (after the fixes of {by})"
        ));
    }

    /// Records what became of each kept proposal and those merged into it:
    /// applied, or declined with the reason. True when something stayed applied.
    fn account(
        &mut self,
        (kept, merged): (&[Proposal], &[(Proposal, Fingerprint)]),
        rolled: &BTreeMap<PathBuf, String>,
        verdict: &Verdict,
        unchanged: &BTreeSet<PathBuf>,
        round: usize,
    ) -> bool {
        let mut any = false;
        for proposal in kept {
            let files = &proposal.files;
            let reason = match files.iter().find(|f| rolled.contains_key(*f)) {
                Some(file) => Some(verdict.own.get(&proposal.fingerprint).map_or_else(
                    || (Class::RolledBack, format!("rolled back: {}", rolled[file])),
                    Clone::clone,
                )),
                None if files.iter().all(|f| unchanged.contains(f)) => {
                    Some((Class::Unchanged, "the fix changes nothing".to_owned()))
                }
                None => None,
            };
            let covered = merged
                .iter()
                .filter(|(_, into)| *into == proposal.fingerprint)
                .map(|(p, _)| p);
            for p in std::iter::once(proposal).chain(covered) {
                let model = p.fixer == FixedBy::Model;
                // A finding the edit of a file did not fix is declined while
                // the edit stands for the others.
                let reason = verdict.own.get(&p.fingerprint).or(reason.as_ref());
                if let Some((class, reason)) = reason {
                    if model {
                        self.proposer
                            .note_model(&p.rule_id, Event::Declined(*class));
                    }
                    self.decline_proposal(p, reason.clone());
                    continue;
                }
                if model {
                    self.proposer.note_model(&p.rule_id, Event::Verified);
                }
                any = true;
                let preview = self
                    .previewable
                    .get(&p.fingerprint)
                    .and_then(|standing| standing.shown.as_ref().ok())
                    .cloned();
                self.applied.push(AppliedFix {
                    fingerprint: p.fingerprint.clone(),
                    rule_id: p.rule_id.clone(),
                    fixer: p.fixer.clone(),
                    description: p.description.clone(),
                    safety: p.safety,
                    files: files.clone(),
                    round,
                    preview,
                    follow_up: false,
                });
            }
        }
        any
    }

    /// Keeps what every standing fix changes, in case the run may not write it
    /// and leaves it for a hand to apply, or why it has none; in a dry run, of
    /// every fix, because none is written. `bases` holds the
    /// texts the round changed; a file it left as it was has the text it
    /// stands in for.
    fn keep_previews(
        &mut self,
        kept: &[Proposal],
        merged: &[(Proposal, Fingerprint)],
        rolled: &BTreeMap<PathBuf, String>,
        bases: &BTreeMap<PathBuf, String>,
    ) {
        let standing: Vec<&Proposal> = kept
            .iter()
            .chain(merged.iter().map(|(p, _)| p))
            .filter(|p| {
                (self.proposer.run.dry_run || !self.permits(p.safety))
                    && !p.files.iter().any(|f| rolled.contains_key(f))
            })
            .collect();
        for p in standing {
            let shown = preview_of(p, |file| {
                bases
                    .get(file)
                    .cloned()
                    .or_else(|| self.standing_text(file))
            })
            .ok_or_else(|| DeclinedFix {
                fingerprint: p.fingerprint.clone(),
                rule_id: p.rule_id.clone(),
                file: p.file.clone(),
                line: p.line,
                reason: "left for review: its change could not be shown as a preview".to_owned(),
            });
            self.previewable.insert(
                p.fingerprint.clone(),
                Standing {
                    proposal: p.clone(),
                    shown,
                },
            );
        }
    }

    /// Declines the fixes by a model that collided with an earlier one of the
    /// round: they stand for nothing, and the user is told.
    fn decline_dropped(&mut self, dropped: &[Proposal]) {
        for p in dropped.iter().filter(|p| p.fixer == FixedBy::Model) {
            self.proposer
                .note_model(&p.rule_id, Event::Declined(Class::RolledBack));
            self.decline_proposal(
                p,
                "its edit collides with another fix of this round, which was kept".to_owned(),
            );
        }
    }

    /// The text `file` has now: the one that stands in for it, else the
    /// one on disk.
    pub(super) fn standing_text(&self, file: &Path) -> Option<String> {
        let root = &self.proposer.engine.ws.root;
        self.texts
            .get(file)
            .cloned()
            .or_else(|| fs::read_to_string(root.join(file)).ok())
    }

    pub(super) fn decline_proposal(&mut self, p: &Proposal, reason: String) {
        self.declined.insert(
            p.fingerprint.clone(),
            DeclinedFix {
                fingerprint: p.fingerprint.clone(),
                rule_id: p.rule_id.clone(),
                file: p.file.clone(),
                line: p.line,
                reason,
            },
        );
    }
}

/// The files whose edits among `kept` are all of fixes that only move
/// declarations, and together would repeat or drop one, with why.
pub(super) fn scattering(
    kept: &[Proposal],
    text: impl Fn(&Path) -> Option<String>,
) -> BTreeMap<PathBuf, String> {
    let moved = moved_files(kept);
    let mut by_file: BTreeMap<&PathBuf, Vec<&TextEdit>> = BTreeMap::new();
    for edit in kept.iter().flat_map(|p| &p.edits) {
        if moved.contains(&edit.file) {
            by_file.entry(&edit.file).or_default().push(edit);
        }
    }
    by_file
        .into_iter()
        .filter_map(|(file, edits)| {
            let before = text(file)?;
            let why = keeps_declarations(&before, &edits).err()?;
            Some((file.clone(), why))
        })
        .collect()
}

/// Proposals in order, dropping the ones that collide with an earlier one. A
/// proposal with exactly the edits of a kept one is covered by it, and
/// returned with the fingerprint it merged into. The ones that collide are
/// returned last.
pub(super) fn kept_proposals(
    proposals: Vec<Proposal>,
) -> (Vec<Proposal>, Vec<(Proposal, Fingerprint)>, Vec<Proposal>) {
    // The order of the edits of each proposal by file and place, found once:
    // two proposals are the same when their edits are, in that order.
    let placed = |p: &Proposal| {
        let mut order: Vec<usize> = (0..p.edits.len()).collect();
        order.sort_by_key(|&n| {
            let e = &p.edits[n];
            (&e.file, e.range.start, e.range.end)
        });
        order
    };
    let mut kept: Vec<(Proposal, Vec<usize>)> = Vec::new();
    let mut merged = Vec::new();
    let mut dropped = Vec::new();
    for proposal in proposals {
        let order = placed(&proposal);
        let same = kept.iter().find(|(k, kept_order)| {
            kept_order.len() == order.len()
                && kept_order
                    .iter()
                    .zip(&order)
                    .all(|(&a, &b)| k.edits[a] == proposal.edits[b])
        });
        if let Some((same, _)) = same {
            merged.push((proposal, same.fingerprint.clone()));
            continue;
        }
        let collides = proposal.edits.iter().any(|e| {
            kept.iter()
                .flat_map(|(k, _)| &k.edits)
                .any(|k| k.file == e.file && overlap(&k.range, &e.range))
        });
        if collides {
            dropped.push(proposal);
        } else {
            kept.push((proposal, order));
        }
    }
    (kept.into_iter().map(|(p, _)| p).collect(), merged, dropped)
}

/// The changed files whose check got worse: more findings of error or warn
/// severity for some rule in a file, wherever in it (a finding that moves to
/// another symbol of the file is not new, as `model::verify`'s `introduced`
/// counts it), or a new gap in the analysis. A finding or a gap that is new in
/// a file that was not changed is charged to the changed files it depends on
/// (see [`Blame`]); a gap with no path condemns them all.
pub(super) fn worse(blame: &Blame<'_>) -> BTreeMap<PathBuf, String> {
    let (before, after) = blame.runs();
    let changed = blame.changed();
    let gaps: BTreeSet<_> = before.incomplete.iter().collect();
    let mut bad: BTreeMap<PathBuf, String> = BTreeMap::new();
    let grown = blame.grown();
    // What a file owes to its own findings comes before what it is charged.
    for (file, rule) in &grown {
        if changed.iter().any(|c| c == file) {
            bad.entry(file.to_path_buf())
                .or_insert_with(|| format!("it introduced a finding of {rule}"));
        }
    }
    for (file, rule) in &grown {
        if changed.iter().any(|c| c == file) {
            continue;
        }
        for cause in blame.charged(file, rule) {
            bad.entry(cause.clone()).or_insert_with(|| {
                format!("it introduced a finding of {rule} in {}", file.display())
            });
        }
    }
    let mut everyone: Option<String> = None;
    for gap in after.incomplete.iter().filter(|g| !gaps.contains(g)) {
        match &gap.path {
            Some(path) if changed.contains(path) => {
                bad.entry(path.clone()).or_insert_with(|| {
                    format!("the file is no longer fully analyzed: {}", gap.reason)
                });
            }
            // A file nobody changed is no one's doing but that of the changed
            // files it has an edge to.
            Some(path) => {
                for cause in blame.reaching(path) {
                    bad.entry(cause.clone()).or_insert_with(|| {
                        format!(
                            "the analysis became incomplete: {}: {}",
                            path.display(),
                            gap.reason
                        )
                    });
                }
            }
            None => {
                everyone.get_or_insert_with(|| {
                    format!("the analysis became incomplete: {}", gap.reason)
                });
            }
        }
    }
    if let Some(reason) = everyone {
        for file in changed {
            bad.entry(file.clone()).or_insert_with(|| reason.clone());
        }
    }
    bad
}

/// `failed` plus the files that go with them, and only those: every file that
/// shares a proposal with a failed one (a fix that spans files is undone as a
/// whole), and every dependent of `blamed` (a file only the safe fixes of a
/// change reached, with the changed files it answers) one of whose causes
/// failed. Fixes that share no file and no cause stay.
pub(super) fn close_over(
    kept: &[Proposal],
    blamed: &BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    mut failed: BTreeMap<PathBuf, String>,
) -> BTreeMap<PathBuf, String> {
    loop {
        let mut grew = false;
        let groups = kept
            .iter()
            .map(|proposal| (proposal.files.as_slice(), None))
            .chain(
                blamed
                    .iter()
                    .map(|(file, causes)| (std::slice::from_ref(file), Some(causes))),
            );
        let mut joins: Vec<(PathBuf, String)> = Vec::new();
        for (files, causes) in groups {
            let reason = match causes {
                None => files.iter().find_map(|f| failed.get(f)),
                Some(causes) => causes.iter().find_map(|c| failed.get(c)),
            };
            let Some(reason) = reason else {
                continue;
            };
            joins.extend(files.iter().map(|f| (f.clone(), reason.clone())));
        }
        for (file, reason) in joins {
            if let std::collections::btree_map::Entry::Vacant(slot) = failed.entry(file) {
                slot.insert(reason);
                grew = true;
            }
        }
        if !grew {
            return failed;
        }
    }
}

#[cfg(test)]
mod tests;
