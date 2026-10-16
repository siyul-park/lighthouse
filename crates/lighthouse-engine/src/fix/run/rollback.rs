//! A round's verdict: which proposals apply together, which got a file worse
//! and are rolled back whole, and what became of each.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::{Fingerprint, Severity, hash};

use super::{
    super::edits::overlap,
    super::{AppliedFix, DeclinedFix},
    Orchestrator, Proposal,
};
use crate::{Error, Run};

impl Orchestrator<'_> {
    /// Applies what does not collide to the texts in memory, formats, verifies
    /// them with a check over overlays and drops what got worse. The check of
    /// the resulting state, when nothing was dropped.
    pub(super) fn apply_round(
        &mut self,
        before: Run,
        mut proposals: Vec<Proposal>,
        round: usize,
    ) -> Result<Option<Run>, Error> {
        // Safe fixes win a collision, so the ones that apply unasked are not
        // shut out by a suggestion.
        proposals.sort_by_key(|p| p.safety);
        let (kept, merged) = kept_proposals(proposals);
        let mut candidates = self.candidates(&kept, &before);
        let mut failed = std::mem::take(&mut candidates.failed);
        self.notes.append(&mut candidates.notes);
        let mut trial = self.texts.clone();
        trial.extend(candidates.texts.clone());
        let after = if failed.len() == kept.len() && candidates.texts.is_empty() {
            None
        } else {
            Some(self.engine.check_overlaid(&[], &[], &trial)?)
        };
        let changed: Vec<PathBuf> = candidates.texts.keys().cloned().collect();
        if let Some(after) = &after {
            failed.extend(worse(&before, after, &changed));
        }
        let rolled = close_over(&kept, failed);
        for (file, reason) in &rolled {
            candidates.texts.remove(file);
            self.notes
                .push(format!("rolled back {}: {reason}", file.display()));
        }
        for (file, text) in &candidates.texts {
            self.original.entry(file.clone()).or_insert_with(|| {
                let base = candidates.bases[file].clone();
                let hash = hash::sha256(&base);
                (base, hash)
            });
            self.texts.insert(file.clone(), text.clone());
        }
        if self.account(&kept, &merged, &rolled, &candidates.unchanged, round) {
            self.rounds = round;
        }
        Ok(if rolled.is_empty() { after } else { None })
    }

    /// Records what became of each kept proposal and those merged into it:
    /// applied, or declined with the reason. True when something stayed applied.
    fn account(
        &mut self,
        kept: &[Proposal],
        merged: &[(Proposal, Fingerprint)],
        rolled: &BTreeMap<PathBuf, String>,
        unchanged: &BTreeSet<PathBuf>,
        round: usize,
    ) -> bool {
        let mut any = false;
        for proposal in kept {
            let files = proposal.files();
            let reason = match files.iter().find(|f| rolled.contains_key(*f)) {
                Some(file) => Some(format!("rolled back: {}", rolled[file])),
                None if files.iter().all(|f| unchanged.contains(f)) => {
                    Some("the fix changes nothing".to_owned())
                }
                None => None,
            };
            let covered = merged
                .iter()
                .filter(|(_, into)| *into == proposal.fingerprint)
                .map(|(p, _)| p);
            for p in std::iter::once(proposal).chain(covered) {
                if let Some(reason) = &reason {
                    self.decline_proposal(p, reason.clone());
                    continue;
                }
                any = true;
                self.applied.push(AppliedFix {
                    fingerprint: p.fingerprint.clone(),
                    rule_id: p.rule_id.clone(),
                    fixer: p.fixer.clone(),
                    description: p.description.clone(),
                    safety: p.safety,
                    files: files.iter().cloned().collect(),
                    round,
                });
            }
        }
        any
    }

    fn decline_proposal(&mut self, p: &Proposal, reason: String) {
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

/// Proposals in order, dropping the ones that collide with an earlier one. A
/// proposal with exactly the edits of a kept one is covered by it, and
/// returned with the fingerprint it merged into.
fn kept_proposals(proposals: Vec<Proposal>) -> (Vec<Proposal>, Vec<(Proposal, Fingerprint)>) {
    let mut kept: Vec<Proposal> = Vec::new();
    let mut merged = Vec::new();
    for proposal in proposals {
        let same = kept.iter().find(|k| {
            let (a, b) = (k.sorted_edits(), proposal.sorted_edits());
            a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x == y)
        });
        if let Some(same) = same {
            merged.push((proposal, same.fingerprint.clone()));
            continue;
        }
        let collides = proposal.edits.iter().any(|e| {
            kept.iter()
                .flat_map(|k| &k.edits)
                .any(|k| k.file == e.file && overlap(&k.range, &e.range))
        });
        if !collides {
            kept.push(proposal);
        }
    }
    (kept, merged)
}

/// The changed files whose check got worse: more findings of error or warn
/// severity for some rule and owner (the symbol, or the file for a finding with
/// none; a fingerprint can change when code moves, its owner does not), or a new
/// gap in the analysis. A change that cannot be pinned on one file (a gap with
/// no path, a finding in an untouched file) condemns them all.
fn worse(before: &Run, after: &Run, changed: &[PathBuf]) -> BTreeMap<PathBuf, String> {
    type Key = (PathBuf, String, String);
    let count = |o: &Run| -> BTreeMap<Key, usize> {
        let mut counts = BTreeMap::new();
        let loud = o
            .diagnostics
            .iter()
            .filter(|d| matches!(d.severity, Severity::Error | Severity::Warn));
        for d in loud {
            let owner = d.symbol.clone().unwrap_or_default();
            *counts
                .entry((d.file.clone(), d.rule_id.clone(), owner))
                .or_insert(0) += 1;
        }
        counts
    };
    let known = count(before);
    let gaps: BTreeSet<_> = before.incomplete.iter().collect();
    let mut bad: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut everyone: Option<String> = None;
    for (key, now) in &count(after) {
        if known.get(key).copied().unwrap_or(0) >= *now {
            continue;
        }
        let (file, rule, _) = key;
        let reason = format!("it introduced a finding of {rule}");
        if changed.contains(file) {
            bad.entry(file.clone()).or_insert(reason);
        } else {
            everyone.get_or_insert_with(|| format!("{reason} in {}", file.display()));
        }
    }
    for gap in after.incomplete.iter().filter(|g| !gaps.contains(g)) {
        match &gap.path {
            Some(path) if changed.contains(path) => {
                bad.entry(path.clone()).or_insert_with(|| {
                    format!("the file is no longer fully analyzed: {}", gap.reason)
                });
            }
            _ => {
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

/// `failed` plus every file that shares a proposal with a failed one: a fix
/// that spans files is undone as a whole.
fn close_over(
    kept: &[Proposal],
    mut failed: BTreeMap<PathBuf, String>,
) -> BTreeMap<PathBuf, String> {
    loop {
        let mut grew = false;
        for proposal in kept {
            let files = proposal.files();
            let Some(reason) = files.iter().find_map(|f| failed.get(f)).cloned() else {
                continue;
            };
            for file in files {
                if let std::collections::btree_map::Entry::Vacant(slot) = failed.entry(file) {
                    slot.insert(reason.clone());
                    grew = true;
                }
            }
        }
        if !grew {
            return failed;
        }
    }
}
