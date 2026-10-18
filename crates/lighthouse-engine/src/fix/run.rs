mod chain;
mod rollback;
mod settle;
mod write;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::{Fingerprint, Preview};
use lighthouse_protocol::hash::Hasher;
use lighthouse_spec::Approval;

use super::{
    AppliedFix, DeclinedFix, FixPlan, FixReport, FixRun, FixedBy, MAX_ROUNDS, PreviewedFix,
    model::run_verify,
    propose::{Phase, Proposal, Proposer},
};
use chain::{Links, Mark};

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
    /// The previews of the fixes by a model that stand applied in `texts`,
    /// by fingerprint: those the run withholds are reported.
    previewable: BTreeMap<Fingerprint, Result<Preview, DeclinedFix>>,
    /// Fixes by a model that were accepted and are left for a hand.
    previews: Vec<PreviewedFix>,
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
            proposer: Proposer::new(self, plan, run).asking_models(),
            texts: Overlays::new(),
            original: BTreeMap::new(),
            states: BTreeSet::new(),
            applied: Vec::new(),
            declined: BTreeMap::new(),
            notes: Vec::new(),
            rounds: 0,
            previewable: BTreeMap::new(),
            previews: Vec::new(),
        };
        let outcome = orchestrator
            .rounds()
            .and_then(|outcome| orchestrator.model_round(outcome));
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
            let (proposals, declined) =
                self.proposer
                    .propose(&outcome, &self.texts, Phase::Rules, &[])?;
            self.declined.extend(declined);
            if proposals.is_empty() {
                return Ok(outcome);
            }
            let first_applied = self.applied.len();
            checked = self.apply_round(&outcome, proposals, round)?;
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

    /// The round that gives the findings the rules left to a model: its
    /// edits are kept only if the checks accept them, and then written only if
    /// the run may (`--unsafe-fixes`, or `approval.fix = "allow"`) and the
    /// commands of `[fix] verify` pass over them. Otherwise they are
    /// withdrawn to previews; when a command fails, the runs of the chain are
    /// rolled back newest first until they pass, and their findings are
    /// declined. The run of the last check.
    fn model_round(&mut self, outcome: Run) -> Result<Run, Error> {
        let (proposals, declined) =
            self.proposer
                .propose(&outcome, &self.texts, Phase::Models, &[])?;
        self.declined.extend(declined);
        if proposals.is_empty() {
            return Ok(outcome);
        }
        let links: Links = proposals
            .iter()
            .filter(|p| p.fixer == FixedBy::Model)
            .map(|p| (p.fingerprint.clone(), p.chain))
            .collect();
        let start = self.mark();
        let (marks, current) = self.apply_chain(&outcome, proposals, start.2 + 1)?;
        if !self.has_model_fix() {
            return Ok(outcome);
        }
        if !self.permitted() {
            self.withdraw(start);
            return Ok(outcome);
        }
        let mut rolled = false;
        while let Some(why) = self.failing_verify() {
            if !self.roll_back_newest(&outcome, &links, &marks, &why) {
                break;
            }
            rolled = true;
        }
        if !self.has_model_fix() {
            self.rounds = start.2;
            return Ok(outcome);
        }
        match current {
            Some(checked) if !rolled => Ok(checked),
            _ => self.proposer.engine.check_overlaid(&[], &[], &self.texts),
        }
    }

    /// Applies the links of the chain in `proposals` one at a time, each over
    /// what the ones before it left and judged by itself. The state before
    /// each, by link, and the check of the last state that changed.
    fn apply_chain(
        &mut self,
        outcome: &Run,
        proposals: Vec<Proposal>,
        round: usize,
    ) -> Result<(BTreeMap<usize, Mark>, Option<Run>), Error> {
        let mut links: BTreeMap<usize, Vec<Proposal>> = BTreeMap::new();
        for p in proposals {
            links.entry(p.chain).or_default().push(p);
        }
        let mut marks = BTreeMap::new();
        let mut current: Option<Run> = None;
        for (number, link) in links {
            let link = self.made_over_standing(link);
            if link.is_empty() {
                continue;
            }
            let mark = self.mark();
            let before = current.as_ref().unwrap_or(outcome);
            let checked = self.apply_round(before, link, round)?;
            let changed = self.texts != mark.0;
            marks.insert(number, mark);
            if changed {
                current = Some(match checked {
                    Some(checked) => checked,
                    None => self.proposer.engine.check_overlaid(&[], &[], &self.texts)?,
                });
            }
        }
        Ok((marks, current))
    }

    /// Whether the run may write the fixes by a model: `--unsafe-fixes`, or
    /// the approval of the project.
    fn permitted(&self) -> bool {
        self.proposer.run.unsafe_fixes || self.proposer.engine.config.approval() == Approval::Allow
    }

    /// Why the commands of `[fix] verify` do not pass over the texts of the
    /// run, if they do not.
    fn failing_verify(&self) -> Option<String> {
        let engine = self.proposer.engine;
        let commands = &engine.config.fix().verify;
        if commands.is_empty() {
            return None;
        }
        if !engine.trusted {
            return Some(
                "`[fix] verify` runs only in a trusted project (`lighthouse trust`)".to_owned(),
            );
        }
        run_verify(
            &engine.ws.root,
            &self.texts,
            commands,
            engine.config.fix().timeout,
        )
        .err()
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
