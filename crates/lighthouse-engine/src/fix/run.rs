mod chain;
mod rollback;
mod settle;
mod write;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::{Fingerprint, Preview, Safety};
use lighthouse_protocol::hash::Hasher;

use super::{
    AppliedFix, DeclinedFix, FixPlan, FixReport, FixRun, FixedBy, MAX_ROUNDS, PreviewedFix,
    propose::{Phase, Proposal, Proposer, Taking},
    rewrite::run_verify,
};
use chain::{Links, Mark};
use rollback::apart;

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
    /// The fixes that stand applied in `texts`, by fingerprint, with what
    /// they change: those the run withholds are reported and kept.
    previewable: BTreeMap<Fingerprint, Standing>,
    /// `unsafe` fixes that were accepted and are left for a hand.
    previews: Vec<PreviewedFix>,
}

/// A fix that stands applied in the texts of the run.
struct Standing {
    proposal: Proposal,
    /// What it changes, or why that cannot be shown.
    shown: Result<Preview, DeclinedFix>,
}

impl Engine {
    /// Fixes the selected findings of the project: see the module docs. The
    /// analysis must be complete, because a fix that rests on a partial
    /// analysis is a guess.
    pub fn fix(&self, plan: &FixPlan, run: &FixRun) -> Result<FixReport, Error> {
        if let Some(id) = &run.fixer {
            if self.registry.fixer(id).is_none() && plan.command(id).is_none() {
                return Err(Error::Fix(format!("unknown fixer `{id}`")));
            }
            if run.rules.is_empty() && run.fingerprints.is_empty() {
                return Err(Error::Fix(
                    "a named fixer needs the rules or fingerprints it is for".to_owned(),
                ));
            }
        }
        let (cache, kept) = match self.open_cache() {
            Some(Ok(cache)) => (Some(cache), None),
            Some(Err(e)) => (None, Some(format!("the previews are not kept: {e}"))),
            None => (None, None),
        };
        let proposer = Proposer::new(self, plan, run).keeping(cache);
        let mut orchestrator = Orchestrator {
            proposer: proposer.asking_models(),
            texts: Overlays::new(),
            original: BTreeMap::new(),
            states: BTreeSet::new(),
            applied: Vec::new(),
            declined: BTreeMap::new(),
            notes: kept.into_iter().collect(),
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
    /// The rounds that apply what the run may write, then the verification
    /// of what it may not, over the state the rounds left: the run of the last
    /// check. Candidates live in memory only, so an error leaves the disk as it
    /// was.
    fn rounds(&mut self) -> Result<Run, Error> {
        let (outcome, withheld) = self.apply_rules()?;
        let withheld = match withheld {
            Some(withheld) => withheld,
            None => self.withheld(&outcome)?,
        };
        self.preview(&outcome, withheld)?;
        Ok(outcome)
    }

    /// Rounds until one applies nothing, or the run stops; the run of the last
    /// check and, when it ended because nothing was left to apply, the
    /// proposals the run may not write over that state.
    fn apply_rules(&mut self) -> Result<(Run, Option<Vec<Proposal>>), Error> {
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
                    .propose(&outcome, &self.texts, Phase::Rules, Taking::Selected)?;
            self.declined.extend(declined);
            let (proposals, withheld): (Vec<_>, Vec<_>) =
                proposals.into_iter().partition(|p| self.permits(p.safety));
            if proposals.is_empty() {
                return Ok((outcome, Some(withheld)));
            }
            let first_applied = self.applied.len();
            checked = self.apply_round(&outcome, proposals, round)?;
            if checked.is_none() && self.rounds == round - 1 {
                // Everything this round was rolled back: stop rather than repeat it.
                return Ok((engine.check_overlaid(&[], &[], &self.texts)?, None));
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
        let outcome = match checked {
            Some(outcome) => outcome,
            None => engine.check_overlaid(&[], &[], &self.texts)?,
        };
        Ok((outcome, None))
    }

    /// The proposals over the state the rounds left that the run may not
    /// write.
    fn withheld(&mut self, outcome: &Run) -> Result<Vec<Proposal>, Error> {
        let (proposals, declined) =
            self.proposer
                .propose(outcome, &self.texts, Phase::Rules, Taking::Selected)?;
        self.declined.extend(declined);
        Ok(proposals
            .into_iter()
            .filter(|p| !self.permits(p.safety))
            .collect())
    }

    /// Verifies the `withheld` fixes the run may not write, over the state the
    /// rounds left, and reports those the checks accept as previews. The texts
    /// are left as they were. A fix is applied later by itself, so those that
    /// edit the same file are verified in rounds of their own: each preview
    /// carries the safe fixes of its own change.
    fn preview(&mut self, outcome: &Run, withheld: Vec<Proposal>) -> Result<(), Error> {
        for batch in apart(withheld) {
            let start = self.mark();
            self.apply_round(outcome, batch, self.rounds + 1)?;
            self.withdraw(start);
        }
        Ok(())
    }

    /// The round that gives the findings the rules left to a model: its
    /// edits are kept only if the checks accept them. Those the run may write
    /// (a `safe` fix, `--unsafe-fixes`, or `[fix] unsafe`) are written if the
    /// programs of `[fix] verify` pass over them; when a command fails, the
    /// runs of the chain are rolled back newest first until they pass, and
    /// their findings are declined. The others are verified over what is
    /// written and withdrawn to previews. The run of the last check.
    fn model_round(&mut self, outcome: Run) -> Result<Run, Error> {
        let (proposals, declined) =
            self.proposer
                .propose(&outcome, &self.texts, Phase::Models, Taking::Selected)?;
        self.declined.extend(declined);
        let (written, withheld): (Vec<_>, Vec<_>) =
            proposals.into_iter().partition(|p| self.permits(p.safety));
        let outcome = self.write_models(outcome, written)?;
        if !withheld.is_empty() {
            let start = self.mark();
            self.apply_chain(&outcome, withheld, start.rounds + 1)?;
            self.withdraw(start);
        }
        Ok(outcome)
    }

    /// Applies the `proposals` of a model the run may write, and keeps those
    /// the programs of `[fix] verify` pass over. The run of the last check.
    fn write_models(&mut self, outcome: Run, proposals: Vec<Proposal>) -> Result<Run, Error> {
        if proposals.is_empty() {
            return Ok(outcome);
        }
        let links: Links = proposals
            .iter()
            .filter(|p| p.fixer == FixedBy::Model)
            .map(|p| (p.fingerprint.clone(), p.chain))
            .collect();
        let start = self.mark();
        let (marks, current) = self.apply_chain(&outcome, proposals, start.rounds + 1)?;
        if !self.has_model_fix() {
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
            self.rounds = start.rounds;
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
            let changed = self.texts != mark.texts;
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

    /// Whether the run may write a fix of this safety, whoever makes it: a
    /// `safe` fix always, an `unsafe` one as the run asks (`--unsafe-fixes`,
    /// `--no-unsafe-fixes`) or else as the project's `[fix] unsafe` says.
    fn permits(&self, safety: Safety) -> bool {
        safety == Safety::Safe
            || self
                .proposer
                .run
                .unsafe_fixes
                .unwrap_or(self.proposer.engine.config.fix().unsafe_fixes)
    }

    /// Why the programs of `[fix] verify` do not pass over the texts of the
    /// run, if they do not.
    fn failing_verify(&self) -> Option<String> {
        let engine = self.proposer.engine;
        let programs = &engine.config.fix().verify;
        if programs.is_empty() {
            return None;
        }
        if !engine.trusted {
            return Some(
                "`[fix] verify` runs only in a trusted project (`lighthouse trust`)".to_owned(),
            );
        }
        run_verify(&engine.ws.root, &self.texts, programs, &engine.ws.sandbox).err()
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
