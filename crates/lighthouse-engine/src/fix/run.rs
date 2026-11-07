mod blame;
mod chain;
mod cycle;
mod displaced;
mod incomplete;
mod rollback;
mod settle;
mod withheld;
mod write;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::Instant,
};

use lighthouse_model::{Fingerprint, Preview, Safety};
use lighthouse_protocol::hash::Hasher;

use super::{
    AppliedFix, DeclinedFix, FixPlan, FixReport, FixRun, FixedBy, MAX_ROUNDS, PreviewedFix,
    propose::{Phase, Proposal, Proposer, Taking},
    rewrite::run_verify,
};
use chain::{Links, Mark};
use cycle::Trail;

use crate::{Engine, Error, FixTimings, Outcome, Overlays};

/// The state of one fix run.
pub(super) struct Orchestrator<'e> {
    proposer: Proposer<'e>,
    /// The text of every file that stands in for the disk: nothing is written
    /// until the last round has been verified.
    texts: Overlays,
    /// The first text of every file the run changed, with its hash: what the
    /// disk must still hold when the run writes.
    original: BTreeMap<PathBuf, (String, String)>,
    /// The texts each file passed through, to stop an oscillation.
    trail: Trail,
    applied: Vec<AppliedFix>,
    declined: BTreeMap<Fingerprint, DeclinedFix>,
    notes: Vec<String>,
    rounds: usize,
    /// The fixes that stand applied in `texts`, by fingerprint, with what
    /// they change: those the run withholds are reported and kept.
    previewable: BTreeMap<Fingerprint, Standing>,
    /// `unsafe` fixes that were accepted and are left for a hand.
    previews: Vec<PreviewedFix>,
    /// Where the time went.
    timings: FixTimings,
    /// The round the checks being made belong to.
    round: usize,
}

/// A fix that stands applied in the texts of the run.
struct Standing {
    proposal: Proposal,
    /// What it changes, or why that cannot be shown.
    shown: Result<Preview, DeclinedFix>,
}

impl Orchestrator<'_> {
    /// A check of the project over the texts of the run, timed.
    pub(super) fn recheck(&mut self) -> Result<Outcome, Error> {
        let started = Instant::now();
        let run = self.proposer.engine.check_overlaid(&[], &[], &self.texts);
        self.timings.checks.push((self.round, started.elapsed()));
        run
    }

    /// The proposals of the fixers over the texts of the run, timed.
    pub(super) fn proposals(
        &mut self,
        outcome: &Outcome,
        phase: Phase,
        taking: Taking<'_>,
    ) -> Result<(Vec<Proposal>, BTreeMap<Fingerprint, DeclinedFix>), Error> {
        let started = Instant::now();
        let proposed = self.proposer.propose(outcome, &self.texts, phase, taking);
        self.timings.propose += started.elapsed();
        let (proposals, mut declined) = proposed?;
        let (proposals, refused): (Vec<_>, Vec<_>) = proposals
            .into_iter()
            .map(|p| (incomplete::refusal(outcome), p))
            .partition(|(why, _)| why.is_none());
        for (why, p) in refused {
            declined.insert(
                p.fingerprint.clone(),
                DeclinedFix {
                    fingerprint: p.fingerprint,
                    rule_id: p.rule_id,
                    file: p.file,
                    line: p.line,
                    reason: why.unwrap_or_default(),
                },
            );
        }
        Ok((proposals.into_iter().map(|(_, p)| p).collect(), declined))
    }

    /// The rounds that apply what the run may write, then the verification
    /// of what it may not, over the state the rounds left: the run of the last
    /// check. Candidates live in memory only, so an error leaves the disk as it
    /// was.
    fn rounds(&mut self) -> Result<Outcome, Error> {
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
    fn apply_rules(&mut self) -> Result<(Outcome, Option<Vec<Proposal>>), Error> {
        let mut checked: Option<Outcome> = self.proposer.run.initial.clone();
        // The states of the texts the rounds began from: one that comes again
        // means the rounds go round.
        let mut seen = BTreeSet::new();
        for round in 1..=MAX_ROUNDS {
            self.round = round;
            let outcome = match checked.take() {
                Some(outcome) => outcome,
                None => self.recheck()?,
            };
            let outcome = self.watch(outcome, round)?;
            let start = self.state_hash();
            if !seen.insert(start.clone()) {
                self.notes.push(format!(
                    "stopped after {} rounds: the texts are as they were at the start of an earlier round",
                    round - 1
                ));
                return Ok((outcome, None));
            }
            let (proposals, declined) = self.proposals(&outcome, Phase::Rules, Taking::Selected)?;
            self.declined.extend(declined);
            let proposals = self.unfrozen(proposals);
            let (proposals, withheld): (Vec<_>, Vec<_>) =
                proposals.into_iter().partition(|p| self.permits(p.safety));
            if proposals.is_empty() {
                return Ok((outcome, Some(withheld)));
            }
            checked = self.apply_round(&outcome, proposals, round)?;
            if checked.is_none() && self.rounds == round - 1 {
                // Everything this round was rolled back: stop rather than repeat it.
                // The texts are those `outcome` checked, so it is their check.
                if self.state_hash() == start {
                    return Ok((outcome, None));
                }
                return Ok((self.recheck()?, None));
            }
            if round == MAX_ROUNDS {
                self.notes.push(format!(
                    "stopped at the cap of {MAX_ROUNDS} rounds; run it again to continue"
                ));
            }
        }
        self.round = MAX_ROUNDS + 1;
        let outcome = match checked {
            Some(outcome) => outcome,
            None => self.recheck()?,
        };
        Ok((self.watch(outcome, MAX_ROUNDS + 1)?, None))
    }

    /// The proposals over the state the rounds left that the run may not
    /// write.
    fn withheld(&mut self, outcome: &Outcome) -> Result<Vec<Proposal>, Error> {
        let (proposals, declined) = self.proposals(outcome, Phase::Rules, Taking::Selected)?;
        self.declined.extend(declined);
        Ok(self
            .unfrozen(proposals)
            .into_iter()
            .filter(|p| !self.permits(p.safety))
            .collect())
    }

    /// The round that gives the findings the rules left to a model: its
    /// edits are kept only if the checks accept them. Those the run may write
    /// (a `safe` fix, `--unsafe-fixes`, or `[fix] unsafe`) are written if the
    /// programs of `[fix] verify` pass over them; when a command fails, the
    /// runs of the chain are rolled back newest first until they pass, and
    /// their findings are declined. The others are verified over what is
    /// written and withdrawn to previews. The run of the last check.
    fn model_round(&mut self, outcome: Outcome) -> Result<Outcome, Error> {
        let (proposals, declined) = self.proposals(&outcome, Phase::Models, Taking::Selected)?;
        self.declined.extend(declined);
        let (written, withheld): (Vec<_>, Vec<_>) = self
            .unfrozen(proposals)
            .into_iter()
            .partition(|p| self.permits(p.safety));
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
    fn write_models(
        &mut self,
        outcome: Outcome,
        proposals: Vec<Proposal>,
    ) -> Result<Outcome, Error> {
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
            _ => self.recheck(),
        }
    }

    /// Applies the links of the chain in `proposals` one at a time, each over
    /// what the ones before it left and judged by itself. The state before
    /// each, by link, and the check of the last state that changed.
    fn apply_chain(
        &mut self,
        outcome: &Outcome,
        proposals: Vec<Proposal>,
        round: usize,
    ) -> Result<(BTreeMap<usize, Mark>, Option<Outcome>), Error> {
        let mut links: BTreeMap<usize, Vec<Proposal>> = BTreeMap::new();
        for p in proposals {
            links.entry(p.chain).or_default().push(p);
        }
        let mut marks = BTreeMap::new();
        let mut current: Option<Outcome> = None;
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
                    None => self.recheck()?,
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
        run_verify(&engine.files, &self.texts, programs, &engine.ws.sandbox).err()
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

/// Fixes the selected findings of the project: see the module docs. The
/// analysis must be complete, because a fix that rests on a partial
/// analysis is a guess.
pub(crate) fn fix(engine: &Engine, plan: &FixPlan, run: &FixRun) -> Result<FixReport, Error> {
    if let Some(id) = &run.fixer {
        if engine.registry.fixer(id).is_none() && plan.command(id).is_none() {
            return Err(Error::Fix(format!("unknown fixer `{id}`")));
        }
        if run.rules.is_empty() && run.fingerprints.is_empty() {
            return Err(Error::Fix(
                "a named fixer needs the rules or fingerprints it is for".to_owned(),
            ));
        }
    }
    let proposer = Proposer::new(engine, plan, run).keeping(engine.cache.is_some());
    let mut orchestrator = Orchestrator {
        proposer: proposer.asking_models(),
        texts: Overlays::new(),
        original: BTreeMap::new(),
        trail: Trail::default(),
        applied: Vec::new(),
        declined: BTreeMap::new(),
        notes: Vec::new(),
        rounds: 0,
        previewable: BTreeMap::new(),
        previews: Vec::new(),
        timings: FixTimings::default(),
        round: 0,
    };
    let outcome = orchestrator
        .rounds()
        .and_then(|outcome| orchestrator.model_round(outcome));
    let report = orchestrator.write(outcome);
    let trimmed = crate::cache::fit(engine.cache.as_ref(), engine.lock_graph().as_mut());
    report.map(|mut report| {
        report.notes.extend(trimmed);
        report
    })
}
