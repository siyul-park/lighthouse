mod rollback;
mod write;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::Fingerprint;
use lighthouse_model::hash::Hasher;

use super::{AppliedFix, DeclinedFix, FixPlan, FixReport, FixRun, MAX_ROUNDS, propose::Proposer};

use crate::{Engine, Error, Overlays, Run};

/// The state of one fix run.
pub(super) struct Orchestrator<'e> {
    proposer: Proposer<'e>,
    /// The text of every file that stands in for the disk: nothing is written
    /// until the last round has been verified.
    texts: Overlays,
    /// The first text of every file the run changed, with its hash: what the
    /// disk must still hold when the run writes.
    original: BTreeMap<PathBuf, (String, String)>,
    /// Hashes of the states the rounds reached, to stop an oscillation.
    states: BTreeSet<String>,
    applied: Vec<AppliedFix>,
    declined: BTreeMap<Fingerprint, DeclinedFix>,
    notes: Vec<String>,
    rounds: usize,
}

impl Engine {
    /// Fixes the selected findings of the project: see the module docs. The
    /// analysis must be complete, because a fix that rests on a partial
    /// analysis is a guess.
    pub fn fix(&self, plan: &FixPlan, run: &FixRun) -> Result<FixReport, Error> {
        if let Some(id) = &run.fixer {
            if self.registry.fixer(id).is_none() {
                return Err(Error::Fix(format!("unknown fixer `{id}`")));
            }
            if run.rules.is_empty() && run.fingerprints.is_empty() {
                return Err(Error::Fix(
                    "a named fixer needs the rules or fingerprints it is for".to_owned(),
                ));
            }
        }
        let mut orchestrator = Orchestrator {
            proposer: Proposer::new(self, plan, run),
            texts: Overlays::new(),
            original: BTreeMap::new(),
            states: BTreeSet::new(),
            applied: Vec::new(),
            declined: BTreeMap::new(),
            notes: Vec::new(),
            rounds: 0,
        };
        let outcome = orchestrator.rounds();
        orchestrator.write(outcome)
    }
}

impl Orchestrator<'_> {
    /// Rounds until one applies nothing; the run of the last check.
    /// Candidates live in memory only, so an error leaves the disk as it was.
    fn rounds(&mut self) -> Result<Run, Error> {
        let engine = self.proposer.engine;
        let mut checked: Option<Run> = None;
        self.states.insert(self.state_hash());
        for round in 1..=MAX_ROUNDS {
            let outcome = match checked.take() {
                Some(outcome) => outcome,
                None => engine.check_overlaid(&[], &[], &self.texts)?,
            };
            if round == 1 && !outcome.incomplete.is_empty() {
                let first = &outcome.incomplete[0];
                return Err(Error::Fix(format!(
                    "the analysis is incomplete ({}{}), so nothing was fixed",
                    first
                        .path
                        .as_ref()
                        .map_or(String::new(), |p| format!("{}: ", p.display())),
                    first.reason
                )));
            }
            let (proposals, declined) = self.proposer.propose(&outcome, &self.texts)?;
            self.declined.extend(declined);
            if proposals.is_empty() {
                return Ok(outcome);
            }
            let first_applied = self.applied.len();
            checked = self.apply_round(outcome, proposals, round)?;
            if checked.is_none() && self.rounds == round - 1 {
                // Everything this round was rolled back: stop rather than repeat it.
                return engine.check_overlaid(&[], &[], &self.texts);
            }
            if !self.states.insert(self.state_hash()) {
                let mut rules: Vec<&str> = self.applied[first_applied..]
                    .iter()
                    .map(|a| a.rule_id.as_str())
                    .collect();
                rules.sort_unstable();
                rules.dedup();
                self.notes.push(format!(
                    "stopped: the fixes of {} undo each other",
                    rules.join(", ")
                ));
                break;
            }
            if round == MAX_ROUNDS {
                self.notes.push(format!(
                    "stopped after {MAX_ROUNDS} rounds; run it again to continue"
                ));
            }
        }
        match checked {
            Some(outcome) => Ok(outcome),
            None => engine.check_overlaid(&[], &[], &self.texts),
        }
    }

    /// A hash of every text the run holds, which two rounds share only when
    /// the second undid the first.
    fn state_hash(&self) -> String {
        let mut hash = Hasher::new();
        for (path, text) in &self.texts {
            if self
                .original
                .get(path)
                .is_some_and(|(first, _)| first == text)
            {
                continue;
            }
            hash.update(path.to_string_lossy().as_bytes());
            hash.update([0]);
            hash.update(text.as_bytes());
            hash.update([0xff]);
        }
        hash.hex()
    }
}
