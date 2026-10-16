mod rollback;
mod write;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::hash::Hasher;
use lighthouse_model::{Capability, Diagnostic, EditOp, Fingerprint, FixOutcome, Safety};
use lighthouse_plugin::{FixDecision, FixRequest};
use serde_json::Value;

use super::{
    AppliedFix, DeclinedFix, FixBinding, FixPlan, FixReport, FixRun, MAX_ROUNDS,
    edits::{TextEdit, self_overlap},
    lower::{Lowerer, Sources},
};

use crate::{Engine, Error, Overlays, Run, engine::Scopes};

/// A finding's fix, lowered and ready to apply.
pub(super) struct Proposal {
    fingerprint: Fingerprint,
    rule_id: String,
    /// Where the finding is, for reports.
    file: PathBuf,
    line: u32,
    fixer: String,
    pub(super) description: String,
    pub(super) safety: Safety,
    pub(super) edits: Vec<TextEdit>,
}

impl Proposal {
    fn files(&self) -> BTreeSet<PathBuf> {
        self.edits.iter().map(|e| e.file.clone()).collect()
    }

    fn sorted_edits(&self) -> Vec<&TextEdit> {
        let mut edits: Vec<&TextEdit> = self.edits.iter().collect();
        edits.sort_by(|a, b| {
            (&a.file, &a.range.start, &a.range.end).cmp(&(&b.file, &b.range.start, &b.range.end))
        });
        edits
    }
}

/// The state of one fix run.
pub(super) struct Orchestrator<'e> {
    engine: &'e Engine,
    plan: &'e FixPlan,
    run: &'e FixRun,
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
        let mut orchestrator = Orchestrator::new(self, plan, run);
        let outcome = orchestrator.rounds();
        orchestrator.finish(outcome)
    }
}

impl<'e> Orchestrator<'e> {
    /// A run that has changed nothing yet. A preview only proposes with it:
    /// it holds no text and writes nothing.
    pub(super) fn new(engine: &'e Engine, plan: &'e FixPlan, run: &'e FixRun) -> Self {
        Self {
            engine,
            plan,
            run,
            texts: Overlays::new(),
            original: BTreeMap::new(),
            states: BTreeSet::new(),
            applied: Vec::new(),
            declined: BTreeMap::new(),
            notes: Vec::new(),
            rounds: 0,
        }
    }
}

impl Orchestrator<'_> {
    /// Rounds until one applies nothing; the outcome of the last check.
    /// Candidates live in memory only, so an error leaves the disk as it was.
    fn rounds(&mut self) -> Result<Run, Error> {
        let mut checked: Option<Run> = None;
        self.states.insert(self.state_hash());
        for round in 1..=MAX_ROUNDS {
            let outcome = match checked.take() {
                Some(outcome) => outcome,
                None => self.engine.check_overlaid(&[], &[], &self.texts)?,
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
            let proposals = self.propose(&outcome)?;
            if proposals.is_empty() {
                return Ok(outcome);
            }
            let first_applied = self.applied.len();
            checked = self.apply_round(outcome, proposals, round)?;
            if checked.is_none() && self.rounds == round - 1 {
                // Everything this round was rolled back: stop rather than repeat it.
                return self.engine.check_overlaid(&[], &[], &self.texts);
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
            None => self.engine.check_overlaid(&[], &[], &self.texts),
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

    /// The proposals this round can try, with the findings that cannot be
    /// fixed recorded as declined.
    fn propose(&mut self, outcome: &Run) -> Result<Vec<Proposal>, Error> {
        let texts = self.texts.clone();
        let sources = Sources::new(&self.engine.ws.root, &outcome.project, &texts);
        let engine = self.engine;
        let complete = |language: &str| engine.provides(language, Capability::CompleteReferences);
        let lowerer = Lowerer::new(&outcome.project, &sources, &complete);
        let mut proposals = Vec::new();
        let candidates = self.select(outcome)?;
        for finding in candidates {
            let Some(binding) = self.binding(&finding.rule_id) else {
                self.decline(finding, "the rule has no fix".to_owned());
                continue;
            };
            // Eligibility comes first: a fix that would not be applied is never
            // even asked for, so a command never runs for nothing.
            if let Some(why) = &binding.unsupported {
                self.decline(finding, why.clone());
                continue;
            }
            if let Some(why) = self.ineligible(&binding) {
                self.decline(finding, why);
                continue;
            }
            match self.propose_one(outcome, finding, &binding, &lowerer, &sources) {
                Ok(proposal) if self.run.unsafe_fixes || proposal.safety == Safety::Safe => {
                    proposals.push(proposal);
                }
                Ok(proposal) => {
                    let reason = format!(
                        "the fixer proposes a {} fix ({}); it is applied only with unsafe fixes (`--unsafe-fixes`, MCP `unsafeFixes`)",
                        proposal.safety, proposal.description
                    );
                    self.decline(finding, reason);
                }
                Err(reason) => self.decline(finding, reason),
            }
        }
        Ok(proposals)
    }

    /// Why a fix may not be applied in this run, if it may not: judged from
    /// the binding alone.
    pub(super) fn ineligible(&self, binding: &FixBinding) -> Option<String> {
        if self.run.unsafe_fixes {
            return None;
        }
        let why = if binding.cap == Safety::Suggested {
            format!("its fix is {}", binding.cap)
        } else if !binding.mechanical {
            "its rule is not mechanical".to_owned()
        } else {
            return None;
        };
        Some(format!(
            "{why}, so it is applied only with unsafe fixes (`--unsafe-fixes`, MCP `unsafeFixes`)"
        ))
    }

    fn decline(&mut self, finding: &Diagnostic, reason: String) {
        self.declined.insert(
            finding.fingerprint.clone(),
            DeclinedFix {
                fingerprint: finding.fingerprint.clone(),
                rule_id: finding.rule_id.clone(),
                file: finding.file.clone(),
                line: finding.span.start.line,
                reason,
            },
        );
    }

    /// The findings the run was asked to fix. A finding of a rule without a
    /// fix is left out unless the selection names it.
    fn select<'o>(&self, outcome: &'o Run) -> Result<Vec<&'o Diagnostic>, Error> {
        let (scopes, _) = Scopes::new(&self.run.paths, &self.engine.ws.root)?;
        let explicit = !self.run.rules.is_empty() || !self.run.fingerprints.is_empty();
        Ok(outcome
            .diagnostics
            .iter()
            .filter(|d| scopes.contains(&d.file))
            .filter(|d| self.run.rules.is_empty() || self.run.rules.contains(&d.rule_id))
            .filter(|d| {
                self.run.fingerprints.is_empty()
                    || self
                        .run
                        .fingerprints
                        .iter()
                        .any(|p| d.fingerprint.as_str().starts_with(p.as_str()))
            })
            .filter(|d| !self.run.skip.contains(&d.fingerprint))
            .filter(|d| explicit || self.binding(&d.rule_id).is_some())
            .collect())
    }

    fn binding(&self, rule: &str) -> Option<FixBinding> {
        match &self.run.fixer {
            Some(fixer) => Some(FixBinding {
                fixer: fixer.clone(),
                cap: Safety::Suggested,
                mechanical: false,
                unsupported: None,
                decision: FixDecision {
                    id: rule.to_owned(),
                    requirement: String::new(),
                    context: String::new(),
                },
            }),
            None => self.plan.get(rule).cloned(),
        }
    }

    pub(super) fn propose_one(
        &self,
        outcome: &Run,
        finding: &Diagnostic,
        binding: &FixBinding,
        lowerer: &Lowerer,
        sources: &Sources,
    ) -> Result<Proposal, String> {
        let engine = self.engine;
        let fixer = engine
            .registry
            .fixer(&binding.fixer)
            .ok_or_else(|| format!("fixer `{}` is not registered", binding.fixer))?;
        let file = outcome.project.file(&finding.file);
        let language = file.map_or("", |f| f.lang.as_str());
        let provided = engine
            .language(language)
            .map(|provider| provider.manifest().capabilities.as_slice());
        if !engine.provides(language, Capability::Overlays) {
            return Err(format!(
                "the provider of `{language}` does not analyze overlays, so an edit cannot be verified before it is written"
            ));
        }
        if let Some(missing) = fixer
            .manifest()
            .requires
            .iter()
            .find(|c| !provided.is_some_and(|p| p.contains(c)))
        {
            return Err(format!(
                "language `{language}` lacks the capability `{missing}` the fix needs"
            ));
        }
        let text = sources.text(&finding.file)?;
        let facts = outcome
            .facts
            .get(&finding.fingerprint)
            .unwrap_or(&Value::Null);
        let options = outcome
            .options
            .get(&finding.fingerprint)
            .cloned()
            .unwrap_or_default();
        let request = FixRequest {
            finding,
            facts,
            decision: &binding.decision,
            options: &options,
            project: &outcome.project,
            workspace: &engine.ws,
            text: &text,
            keys: &engine.registry,
            trusted: engine.trusted,
        };
        match fixer
            .fix(&request)
            .map_err(|e| format!("the fixer failed: {e}"))?
        {
            FixOutcome::Declined { reason } => Err(reason),
            FixOutcome::Proposed {
                description,
                ops,
                safety,
            } => {
                let mut edits = Vec::new();
                for op in &ops {
                    edits.extend(
                        lowerer
                            .lower(op)
                            .map_err(|why| format!("{}: {why}", op_name(op)))?,
                    );
                }
                if edits.is_empty() {
                    return Err("the fix changes nothing".to_owned());
                }
                if self_overlap(&edits) {
                    return Err("the edits of the fix overlap each other".to_owned());
                }
                Ok(Proposal {
                    fingerprint: finding.fingerprint.clone(),
                    rule_id: finding.rule_id.clone(),
                    file: finding.file.clone(),
                    line: finding.span.start.line,
                    fixer: binding.fixer.clone(),
                    description,
                    safety: safety.max(binding.cap),
                    edits,
                })
            }
        }
    }
}

fn op_name(op: &EditOp) -> &'static str {
    match op {
        EditOp::Move { .. } => "move",
        EditOp::Delete { .. } => "delete",
        EditOp::Reorder { .. } => "reorder",
        EditOp::Rename { .. } => "rename",
        EditOp::DeleteRange { .. } => "delete_range",
        EditOp::Replace { .. } => "replace",
    }
}
