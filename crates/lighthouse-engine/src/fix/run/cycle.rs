//! Oscillation: fixes that undo each other. A file whose text comes back to one
//! it had before is put back to the state with the fewest errors it passed
//! through, and no fix is proposed for it again; the decisions whose fixes were
//! applied between the two equal texts are named in a note, and their findings
//! left in the file are declined.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::Severity;
use lighthouse_protocol::hash;

use super::{super::propose::Proposal, DeclinedFix, Orchestrator};
use crate::{Error, Run};

/// The texts the files of a run passed through.
#[derive(Default)]
pub(super) struct Trail {
    visits: BTreeMap<PathBuf, Vec<Visit>>,
    /// Errors per file in the text the run started from.
    errors: BTreeMap<PathBuf, usize>,
    /// Files whose fixes undid each other: they are left as they were put back.
    frozen: BTreeSet<PathBuf>,
}

/// A text a file had after a round (round 0 is the original), with the errors
/// found in it.
struct Visit {
    hash: String,
    text: String,
    errors: usize,
    round: usize,
}

/// The files whose text came back, with the visit it came back to.
type Returns = Vec<(PathBuf, usize)>;

impl Orchestrator<'_> {
    /// Takes note of the texts of the state `outcome` checked, the one the
    /// rounds before `round` left. A file that came back to a text it had is
    /// put back to its best and frozen; the run of the state then, which is
    /// checked again after that.
    pub(super) fn watch(&mut self, outcome: Run, round: usize) -> Result<Run, Error> {
        let errors = errors_by_file(&outcome);
        if round == 1 {
            self.trail.errors = errors.clone();
        }
        let mut returned = Returns::new();
        for (file, text) in &self.texts {
            if self.trail.frozen.contains(file) {
                continue;
            }
            let Some((first, _)) = self.original.get(file) else {
                continue;
            };
            let baseline = self.trail.errors.get(file).copied().unwrap_or(0);
            let visits = self.trail.visits.entry(file.clone()).or_insert_with(|| {
                vec![Visit {
                    hash: hash::sha256(first),
                    text: first.clone(),
                    errors: baseline,
                    round: 0,
                }]
            });
            let now = hash::sha256(text);
            if visits.last().is_some_and(|v| v.hash == now) {
                continue;
            }
            match visits.iter().position(|v| v.hash == now) {
                Some(at) => returned.push((file.clone(), at)),
                None => visits.push(Visit {
                    hash: now,
                    text: text.clone(),
                    errors: errors.get(file).copied().unwrap_or(0),
                    round: round - 1,
                }),
            }
        }
        if returned.is_empty() {
            return Ok(outcome);
        }
        let cycles: Vec<(PathBuf, Vec<String>)> = returned
            .into_iter()
            .map(|(file, at)| {
                let rules = self.put_back(&file, at);
                (file, rules)
            })
            .collect();
        let outcome = self.proposer.engine.check_overlaid(&[], &[], &self.texts)?;
        for (file, rules) in cycles {
            self.decline_cycle(&outcome, &file, &rules);
        }
        Ok(outcome)
    }

    /// Puts `file`, whose text came back to its visit number `at`, back to the
    /// visit with the fewest errors (the last of them), freezes it and says so.
    /// The decisions whose fixes were applied since the visit `at`.
    fn put_back(&mut self, file: &PathBuf, at: usize) -> Vec<String> {
        let visits = &self.trail.visits[file];
        let (_, best) = visits
            .iter()
            .enumerate()
            .min_by_key(|(index, v)| (v.errors, Reverse(*index)))
            .expect("a file that came back has visits");
        let (text, kept) = (best.text.clone(), best.round);
        let since = visits[at].round;
        let mut rules: Vec<String> = self
            .applied
            .iter()
            .filter(|a| a.round > since && a.files.contains(file))
            .map(|a| a.rule_id.clone())
            .collect();
        rules.sort_unstable();
        rules.dedup();
        self.texts.insert(file.clone(), text);
        for fix in self.applied.iter_mut().filter(|a| a.round > kept) {
            fix.files.retain(|f| f != file);
        }
        self.applied.retain(|a| !a.files.is_empty());
        self.trail.frozen.insert(file.clone());
        self.notes.push(format!(
            "stopped: the fixes of {} undo each other in {}; kept its text of round {kept}, with the fewest errors",
            rules.join(", "),
            file.display()
        ));
        rules
    }

    /// Declines the findings of `rules` that `outcome` still has in `file`.
    fn decline_cycle(&mut self, outcome: &Run, file: &PathBuf, rules: &[String]) {
        let reason = format!("the fixes of {} undo each other", rules.join(", "));
        for d in outcome
            .diagnostics
            .iter()
            .filter(|d| d.file == *file && rules.contains(&d.rule_id))
        {
            self.declined.insert(
                d.fingerprint.clone(),
                DeclinedFix {
                    fingerprint: d.fingerprint.clone(),
                    rule_id: d.rule_id.clone(),
                    file: d.file.clone(),
                    line: d.span.start.line,
                    reason: reason.clone(),
                },
            );
        }
    }

    /// The `proposals` that leave the frozen files alone.
    pub(super) fn unfrozen(&self, proposals: Vec<Proposal>) -> Vec<Proposal> {
        proposals
            .into_iter()
            .filter(|p| !p.files.iter().any(|f| self.trail.frozen.contains(f)))
            .collect()
    }
}

fn errors_by_file(run: &Run) -> BTreeMap<PathBuf, usize> {
    let mut counts = BTreeMap::new();
    for d in run
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
    {
        *counts.entry(d.file.clone()).or_insert(0) += 1;
    }
    counts
}
