//! Proposing fixes: which findings a fix run takes up, the fixer each is
//! given to, and what the fixer proposes, lowered to text edits.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::{
    Diagnostic, Fingerprint, Safety, Unavailable,
    ucm::{Capability, EditOp, FixOutcome},
};
use lighthouse_plugin::{FixDecision, FixRequest, Fixer};
use lighthouse_protocol::hash;
use lighthouse_spec::FixScope;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    DeclinedFix, FixBinding, FixPlan, FixRun, FixedBy,
    edits::{TextEdit, apply, self_overlap},
    kept::NOT_KEPT,
    lower::{Lowerer, Sources},
    model::{Event, ModelFixer, choose_extent},
    rewrite::{Bounds, Change, Changes, World, accept},
};
use crate::{Engine, Error, Needs, Overlays, Run, cache::Cache, engine::Scopes};

/// Why a fix by a model is left to review when the run asks no model.
const NO_MODELS: &str =
    "left for review: models are off (`--no-models`); `review show` gives the brief";

/// A finding's fix, lowered and ready to apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Proposal {
    pub(super) fingerprint: Fingerprint,
    pub(super) rule_id: String,
    /// Where the finding is, for reports.
    pub(super) file: PathBuf,
    pub(super) line: u32,
    pub(super) fixer: FixedBy,
    pub(super) description: String,
    pub(super) safety: Safety,
    /// Which proposal wins when those of a round overlap: the higher.
    pub(super) priority: i32,
    pub(super) edits: Vec<TextEdit>,
    /// The files the edits are in, sorted.
    pub(super) files: Vec<PathBuf>,
    pub(super) aim: Aim,
    /// For a fix by a model, the place of its group in the chain of the
    /// pass: the later the group, the later it ran.
    pub(super) chain: usize,
    /// The digest of each file the edits are in, as the fix was made over it:
    /// the edits apply only over that text.
    pub(super) bases: BTreeMap<PathBuf, String>,
    /// The findings the fix was made for, this one among them: a run of an
    /// agent is for all the findings of a file.
    pub(super) batch: Vec<Fingerprint>,
}

/// What a fix by a model must have achieved for the checks to keep it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Aim {
    /// The finding the proposal is for is gone.
    Finding,
    /// Of the findings of the file that one edit is for, at least one is gone;
    /// those still there are declined, and the edit stands.
    File,
}

/// Whose findings a round proposes fixes for: the rules, which derive a fix
/// from the finding, or a model, which is asked for one only once the rules
/// have had their say.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Rules,
    Models,
}

/// Which findings a call of [`Proposer::propose`] takes up.
#[derive(Clone, Copy)]
pub(super) enum Taking<'a> {
    /// Those the run selected, in the files it fixes.
    Selected,
    /// Whatever the run selected, those of `findings` in `files`: the
    /// findings that a change introduced, for the safe fixes that follow it.
    Introduced {
        files: &'a [PathBuf],
        findings: &'a BTreeSet<Fingerprint>,
    },
}

/// What a fix run asks of fixers: the findings it takes up and the proposals
/// the fixers make of them.
pub(super) struct Proposer<'e> {
    pub(super) engine: &'e Engine,
    pub(super) plan: &'e FixPlan,
    pub(super) run: &'e FixRun,
    /// The fixer of the model fixes; `None` when no model is to be asked.
    pub(super) models: Option<ModelFixer<'e>>,
    /// Where the fixes a run withholds are kept; `None` keeps nothing.
    pub(super) kept: Option<Cache>,
}

impl<'e> Proposer<'e> {
    /// A proposer that asks no model.
    pub(super) fn new(engine: &'e Engine, plan: &'e FixPlan, run: &'e FixRun) -> Self {
        Self {
            engine,
            plan,
            run,
            models: None,
            kept: None,
        }
    }

    /// The same, keeping the fixes the run withholds in `cache`.
    #[must_use]
    pub(super) fn keeping(mut self, cache: Option<Cache>) -> Self {
        self.kept = cache;
        self
    }

    /// The same, asking the agents the project binds.
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
    /// the disk, with the findings that cannot be fixed declined. `taking`
    /// says which findings are taken up.
    pub(super) fn propose(
        &self,
        outcome: &Run,
        texts: &Overlays,
        phase: Phase,
        taking: Taking<'_>,
    ) -> Result<(Vec<Proposal>, BTreeMap<Fingerprint, DeclinedFix>), Error> {
        let sources = Sources::new(&self.engine.ws.root, &outcome.project, texts);
        let engine = self.engine;
        let complete = |language: &str| engine.provides(language, Capability::CompleteReferences);
        let lowerer = Lowerer::new(&outcome.project, &sources, &complete);
        let (mut proposals, mut declined) = (Vec::new(), BTreeMap::new());
        let mut asked_to_edit = Vec::new();
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
        let (scopes, _) = Scopes::new(&self.run.paths, &self.engine.ws.root)?;
        for finding in self.select(outcome, &scopes, taking) {
            let binding = self.binding(&finding.rule_id);
            let model = binding.as_ref().is_some_and(|b| b.fixer == FixedBy::Model);
            if model != (phase == Phase::Models) {
                continue;
            }
            let Some(binding) = binding else {
                decline(finding, "the rule has no fix".to_owned());
                continue;
            };
            // A fix that cannot run here is declined before it is asked for.
            // What the run may write of the rest is settled after the checks.
            if let Err(why) = self.engine.availability().of(&binding.needs) {
                let hint = match why {
                    Unavailable::UntrustedProject => " (run `lighthouse trust`)",
                    _ => "",
                };
                decline(
                    finding,
                    format!("left for review: {why}{hint}; `review show` gives the brief"),
                );
                continue;
            }
            match self.applied_preview(finding, &binding, &sources) {
                Some(Ok(proposal)) => {
                    proposals.push(proposal);
                    continue;
                }
                Some(Err((class, reason))) => {
                    if model {
                        self.note_model(&finding.rule_id, Event::Declined(class));
                    }
                    decline(finding, reason);
                    continue;
                }
                None => {}
            }
            if model && self.models.is_none() {
                decline(finding, NO_MODELS.to_owned());
                continue;
            }
            if model {
                asked_to_edit.push((finding, binding));
                continue;
            }
            let made = match self
                .plan
                .command(self.run.fixer.as_deref().unwrap_or(&finding.rule_id))
            {
                Some(command) => self
                    .propose_command(outcome, finding, (&binding, command), (&sources, texts))
                    .map_err(|(_, reason)| reason),
                None => self.propose_one(outcome, finding, &binding, (&lowerer, &sources, texts)),
            };
            match made {
                Ok(proposal) => proposals.push(proposal),
                Err(reason) => decline(finding, reason),
            }
        }
        proposals.extend(self.propose_edits(
            outcome,
            texts,
            &sources,
            asked_to_edit,
            &mut decline,
        )?);
        Ok((proposals, declined))
    }

    /// The findings the run was asked to fix. A finding of a rule without a
    /// fix is left out unless the selection names it. The findings that a
    /// change introduced are taken up whatever rules the run selected.
    fn select<'o>(
        &self,
        outcome: &'o Run,
        scopes: &Scopes,
        taking: Taking<'_>,
    ) -> Vec<&'o Diagnostic> {
        let introduced = matches!(taking, Taking::Introduced { .. });
        let explicit =
            !introduced && (!self.run.rules.is_empty() || !self.run.fingerprints.is_empty());
        outcome
            .diagnostics
            .iter()
            .filter(|d| scopes.contains(&d.file))
            .filter(|d| match taking {
                Taking::Selected => true,
                Taking::Introduced { files, findings } => {
                    files.contains(&d.file) && findings.contains(&d.fingerprint)
                }
            })
            .filter(|d| {
                introduced || self.run.rules.is_empty() || self.run.rules.contains(&d.rule_id)
            })
            .filter(|d| {
                introduced
                    || self.run.fingerprints.is_empty()
                    || self
                        .run
                        .fingerprints
                        .iter()
                        .any(|p| d.fingerprint.as_str().starts_with(p.as_str()))
            })
            .filter(|d| !self.run.skip.contains(&d.fingerprint))
            .filter(|d| explicit || self.binding(&d.rule_id).is_some())
            .collect()
    }

    fn binding(&self, rule: &str) -> Option<FixBinding> {
        match &self.run.fixer {
            Some(fixer) => Some(FixBinding {
                fixer: FixedBy::Fixer(fixer.clone()),
                cap: Safety::Unsafe,
                languages: Vec::new(),
                // A named command fixer runs under the trust it needs.
                needs: self
                    .plan
                    .get(fixer)
                    .map_or(Needs::Nothing, |named| named.needs.clone()),
                decision: FixDecision {
                    id: rule.to_owned(),
                    requirement: String::new(),
                    context: String::new(),
                },
            }),
            None => self.plan.get(rule).cloned(),
        }
    }

    /// Why an edit of a file in `language` cannot be verified before it is
    /// written, if it cannot.
    pub(super) fn overlay_gap(&self, language: &str) -> Option<String> {
        (!self.engine.provides(language, Capability::Overlays)).then(|| {
            format!(
                "the provider of `{language}` does not analyze overlays, so an edit cannot be verified before it is written"
            )
        })
    }

    pub(super) fn propose_one(
        &self,
        outcome: &Run,
        finding: &Diagnostic,
        binding: &FixBinding,
        (lowerer, sources, texts): (&Lowerer, &Sources, &Overlays),
    ) -> Result<Proposal, String> {
        let engine = self.engine;
        let FixedBy::Fixer(id) = &binding.fixer else {
            return Err("a fix by a model is made by an agent that edits".to_owned());
        };
        let fixer: &dyn Fixer = engine
            .registry
            .fixer(id)
            .ok_or_else(|| format!("fixer `{id}` is not registered"))?;
        let file = outcome.project.file(&finding.file);
        let language = file.map_or("", |f| f.lang.as_str());
        let provided = engine
            .language(language)
            .map(|provider| provider.manifest().capabilities.as_slice());
        if let Some(gap) = self.overlay_gap(language) {
            return Err(gap);
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
                let safety = self.plan.safety(&finding.rule_id, safety, binding.cap);
                if safety == Safety::Unsafe && self.applies_previews() {
                    return Err(NOT_KEPT.to_owned());
                }
                let (edits, bases) = lowered(&ops, lowerer, sources)?;
                let scope = self.plan.scope(&finding.rule_id);
                self.within(scope, outcome, finding, &edits, (sources, texts))?;
                Ok(Proposal {
                    fingerprint: finding.fingerprint.clone(),
                    rule_id: finding.rule_id.clone(),
                    file: finding.file.clone(),
                    line: finding.span.start.line,
                    fixer: binding.fixer.clone(),
                    description,
                    safety,
                    priority: self.plan.priority(&finding.rule_id),
                    edits,
                    files: bases.keys().cloned().collect(),
                    aim: Aim::Finding,
                    chain: 0,
                    bases,
                    batch: vec![finding.fingerprint.clone()],
                })
            }
        }
    }
}

impl Proposer<'_> {
    /// Whether the edits of a fix stay within its scope, else why they do not:
    /// the same check as for the files a command or an agent changed.
    fn within(
        &self,
        scope: FixScope,
        outcome: &Run,
        finding: &Diagnostic,
        edits: &[TextEdit],
        (sources, texts): (&Sources, &Overlays),
    ) -> Result<(), String> {
        if scope == FixScope::Files {
            return Ok(());
        }
        let mut changes = Changes::new();
        for file in edits.iter().map(|e| &e.file).collect::<BTreeSet<_>>() {
            let old = sources.text(file)?;
            let of_file: Vec<&TextEdit> = edits.iter().filter(|e| &e.file == file).collect();
            changes.insert(file.clone(), Change::Text(apply(&old, &of_file)?));
        }
        let extents = if scope == FixScope::Symbol {
            vec![choose_extent(
                &outcome.project,
                finding,
                sources.text(&finding.file)?.as_str(),
            )?]
        } else {
            Vec::new()
        };
        let bounds = Bounds {
            who: "the fix",
            scope,
            file: &finding.file,
            extents: &extents,
        };
        let world = World::new(&self.engine.ws.root, outcome, texts);
        accept(changes, &bounds, &world)
            .map(|_| ())
            .map_err(|(_, why)| why)
    }
}

/// The text edits of `ops`, and the digest of each file they are in as the fix
/// is made over it.
fn lowered(
    ops: &[EditOp],
    lowerer: &Lowerer,
    sources: &Sources,
) -> Result<(Vec<TextEdit>, BTreeMap<PathBuf, String>), String> {
    let mut edits = Vec::new();
    for op in ops {
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
    let mut bases = BTreeMap::new();
    for edit in &edits {
        if !bases.contains_key(&edit.file) {
            bases.insert(
                edit.file.clone(),
                hash::sha256(sources.text(&edit.file)?.as_str()),
            );
        }
    }
    Ok((edits, bases))
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
