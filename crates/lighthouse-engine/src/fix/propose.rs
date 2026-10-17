//! Proposing fixes: which findings a fix run takes up, the fixer each is
//! given to, and what the fixer proposes, lowered to text edits.

use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_model::{
    Diagnostic, Fingerprint, Safety,
    ucm::{Capability, EditOp, FixOutcome},
};
use lighthouse_plugin::{FixDecision, FixRequest, Fixer};
use serde_json::Value;

use super::{
    DeclinedFix, FixBinding, FixPlan, FixRun, MODEL_FIXER,
    edits::{TextEdit, self_overlap},
    lower::{Lowerer, Sources},
    model::{Event, ModelFixer},
};
use crate::{Engine, Error, Overlays, Run, engine::Scopes};

/// A finding's fix, lowered and ready to apply.
pub(super) struct Proposal {
    pub(super) fingerprint: Fingerprint,
    pub(super) rule_id: String,
    /// Where the finding is, for reports.
    pub(super) file: PathBuf,
    pub(super) line: u32,
    pub(super) fixer: String,
    pub(super) description: String,
    pub(super) safety: Safety,
    pub(super) edits: Vec<TextEdit>,
    /// The files the edits are in, sorted.
    pub(super) files: Vec<PathBuf>,
}

/// Whose findings a round proposes fixes for: the rules, which derive a fix
/// from the finding, or a model, which is asked for one only once the rules
/// have had their say.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Rules,
    Models,
}

/// What a fix run asks of fixers: the findings it takes up and the proposals
/// the fixers make of them.
pub(super) struct Proposer<'e> {
    pub(super) engine: &'e Engine,
    plan: &'e FixPlan,
    pub(super) run: &'e FixRun,
    /// The fixer of the model fixes; `None` when no model is to be asked.
    pub(super) models: Option<ModelFixer<'e>>,
}

impl<'e> Proposer<'e> {
    /// A proposer that asks no model.
    pub(super) fn new(engine: &'e Engine, plan: &'e FixPlan, run: &'e FixRun) -> Self {
        Self {
            engine,
            plan,
            run,
            models: None,
        }
    }

    /// The same, asking the models the project binds for chat.
    #[must_use]
    pub(super) fn asking_models(mut self) -> Self {
        self.models = ModelFixer::new(self.engine, &self.plan.models);
        self
    }

    /// Counts `event` for the fix by a model of `decision`.
    pub(super) fn note_model(&self, decision: &str, event: Event) {
        if let Some(models) = &self.models {
            models.account.add(decision, event);
        }
    }

    /// The proposals this round can try over the files as `texts` stand in for
    /// the disk, with the findings that cannot be fixed declined.
    pub(super) fn propose(
        &self,
        outcome: &Run,
        texts: &Overlays,
        phase: Phase,
    ) -> Result<(Vec<Proposal>, BTreeMap<Fingerprint, DeclinedFix>), Error> {
        let sources = Sources::new(&self.engine.ws.root, &outcome.project, texts);
        let engine = self.engine;
        let complete = |language: &str| engine.provides(language, Capability::CompleteReferences);
        let lowerer = Lowerer::new(&outcome.project, &sources, &complete);
        let (mut proposals, mut declined) = (Vec::new(), BTreeMap::new());
        let mut decline = |finding: &Diagnostic, reason: String| {
            declined.insert(
                finding.fingerprint.clone(),
                DeclinedFix {
                    fingerprint: finding.fingerprint.clone(),
                    rule_id: finding.rule_id.clone(),
                    file: finding.file.clone(),
                    line: finding.span.start.line,
                    reason,
                },
            );
        };
        for finding in self.select(outcome)? {
            let binding = self.binding(&finding.rule_id);
            let model = binding.as_ref().is_some_and(|b| b.fixer == MODEL_FIXER);
            if model != (phase == Phase::Models) {
                continue;
            }
            let Some(binding) = binding else {
                decline(finding, "the rule has no fix".to_owned());
                continue;
            };
            // Eligibility comes first: a fix that would not be applied is never
            // even asked for, so a command never runs for nothing. A fix by a
            // model is asked for whatever the run may apply: what it may do
            // with the answer is settled after the checks.
            if let Some(why) = &binding.unsupported {
                decline(finding, why.clone());
                continue;
            }
            if model && self.models.is_none() {
                if !self.plan.models[&finding.rule_id].implicit() {
                    decline(finding, NO_MODEL.to_owned());
                }
                continue;
            }
            if let Some(why) = self.ineligible(&binding).filter(|_| !model) {
                decline(finding, why);
                continue;
            }
            match self.propose_one(outcome, finding, &binding, &lowerer, &sources) {
                Ok(proposal)
                    if model || self.run.unsafe_fixes || proposal.safety == Safety::Safe =>
                {
                    proposals.push(proposal);
                }
                Ok(proposal) => {
                    let reason = format!(
                        "the fixer proposes a {} fix ({}); it is applied only with unsafe fixes (`--unsafe-fixes`, MCP `unsafeFixes`)",
                        proposal.safety, proposal.description
                    );
                    decline(finding, reason);
                }
                Err(reason) => decline(finding, reason),
            }
        }
        Ok((proposals, declined))
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
                runs_command: false,
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
        let fixer: &dyn Fixer = match &self.models {
            Some(models) if binding.fixer == MODEL_FIXER => models,
            _ => engine
                .registry
                .fixer(&binding.fixer)
                .ok_or_else(|| format!("fixer `{}` is not registered", binding.fixer))?,
        };
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
                let mut files: Vec<PathBuf> = edits.iter().map(|e| e.file.clone()).collect();
                files.sort();
                files.dedup();
                Ok(Proposal {
                    fingerprint: finding.fingerprint.clone(),
                    rule_id: finding.rule_id.clone(),
                    file: finding.file.clone(),
                    line: finding.span.start.line,
                    fixer: binding.fixer.clone(),
                    description,
                    safety: safety.max(binding.cap),
                    edits,
                    files,
                })
            }
        }
    }
}

/// Why a fix by a model is not asked for.
const NO_MODEL: &str = "no chat model is available: `[models] chat` is not bound, or model checks are off (`--no-models`)";

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
