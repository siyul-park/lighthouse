//! Proposing fixes: which findings a fix run takes up, the fixer each is
//! given to, and what the fixer proposes, lowered to text edits.
//!
//! The fixes a run leaves for a hand to apply. A fix the run was not permitted
//! to write is kept in the graph with the digests of the files it was
//! verified over, whoever made it: a rule, a command, a plugin or an agent.
//! Applying it later is writing exactly what was shown, if the files are still
//! as they were, and never making the fix again.

use lighthouse_model::ucm::Model;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use lighthouse_graph::{Durability, Graph, Input};
use lighthouse_model::{Diagnostic, Fingerprint, Safety, Unavailable, ucm::Capability};
use lighthouse_plugin::FixDecision;
use lighthouse_protocol::hash;
use lighthouse_spec::Program;
use serde::{Deserialize, Serialize};

use super::{
    DeclinedFix, FixBinding, FixPlan, FixRun, FixedBy,
    edits::TextEdit,
    lower::Sources,
    model::{Class, Event, ModelFixer},
    node::{Ask, FIX, FIXING, Fixed, Fixing, Made},
    program::is_filter,
};
use crate::{Engine, Error, Needs, Outcome, Overlays, engine::Scopes, tools};

/// Why a fix by a model is left to review when the run asks no model.
const NO_MODELS: &str =
    "left for review: models are off (`--no-models`); `review show` gives the brief";

/// Why a fix to apply has no preview kept.
pub(super) const NOT_KEPT: &str = "the preview is not kept (run without --no-cache, or fix again)";

/// The previews of the fixes a run withholds, by fingerprint, as JSON. They
/// are what the user was shown, so the graph keeps them until they have not
/// been read for as long as it keeps anything.
static PREVIEW: Input<String, String> = Input::new("preview").persisted();

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
    /// The edits come from operations that only move declarations, so that the
    /// edits of a round on one file must keep its declarations together.
    pub(super) moves_only: bool,
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
    /// Whether the fixes a run withholds are kept for a later run.
    keeping: bool,
    /// The previews to keep when the run ends, by fingerprint.
    pending: RefCell<Vec<(Fingerprint, String)>>,
    /// The time the programs of command fixes ran.
    pub(super) command_time: Cell<Duration>,
}

impl<'e> Proposer<'e> {
    /// A proposer that asks no model.
    pub(super) fn new(engine: &'e Engine, plan: &'e FixPlan, run: &'e FixRun) -> Self {
        Self {
            engine,
            plan,
            run,
            models: None,
            keeping: false,
            pending: RefCell::default(),
            command_time: Cell::new(Duration::ZERO),
        }
    }

    /// The same, keeping the fixes the run withholds in the graph.
    #[must_use]
    pub(super) fn keeping(mut self, keeping: bool) -> Self {
        self.keeping = keeping;
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
        outcome: &Outcome,
        texts: &Overlays,
        phase: Phase,
        taking: Taking<'_>,
    ) -> Result<(Vec<Proposal>, BTreeMap<Fingerprint, DeclinedFix>), Error> {
        let sources = Sources::new(&self.engine.ws.root, &outcome.project, texts);
        let mut slots = Vec::new();
        let (mut proposals, mut declined) = (Vec::new(), BTreeMap::new());
        let mut asked_to_edit = Vec::new();
        let mut decline = |finding: &Diagnostic, reason: String| {
            declined.insert(finding.fingerprint.clone(), declining(finding, reason));
        };
        let (scopes, _) = Scopes::new(&self.run.paths, &self.engine.ws.root)?;
        let mut asks = Vec::new();
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
            let slot = self.slot(outcome, finding, &binding, (&sources, texts), &mut asks);
            slots.push((finding, binding, slot));
        }
        let fixed = self.pull(outcome, texts, &asks)?;
        for (finding, binding, made) in slots {
            let made = match made {
                Slot::Asked(at) => match fixed[at].as_deref() {
                    Ok(Fixed::Made(made)) => self.assembled(finding, &binding, made),
                    Ok(Fixed::Declined(reason)) | Err(reason) => Err(reason.clone()),
                },
                Slot::Done(made) => *made,
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
        outcome: &'o Outcome,
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

    /// Where the fix of `finding` is made: the position of its ask among
    /// `asks`, else what came of it without the graph.
    fn slot(
        &self,
        outcome: &Outcome,
        finding: &Diagnostic,
        binding: &FixBinding,
        (sources, texts): (&Sources, &Overlays),
        asks: &mut Vec<Ask>,
    ) -> Slot {
        let program = self
            .plan
            .command(self.run.fixer.as_deref().unwrap_or(&finding.rule_id));
        let made = match program {
            Some(command) => self.ask_command(outcome, finding, command, sources),
            None => self.ask_fixer(outcome, finding, binding, sources),
        };
        match (made, program) {
            (Ok(Some(ask)), _) => {
                asks.push(ask);
                Slot::Asked(asks.len() - 1)
            }
            (Ok(None), Some(command)) => Slot::Done(Box::new(
                self.propose_command(outcome, finding, (binding, command), (sources, texts))
                    .map_err(|(_, reason)| reason),
            )),
            (Ok(None), None) => Slot::Done(Box::new(Err("the fix has no command".to_owned()))),
            (Err(reason), _) => Slot::Done(Box::new(Err(reason))),
        }
    }

    /// What a fix by a registered fixer is asked, or why it cannot be.
    fn ask_fixer(
        &self,
        outcome: &Outcome,
        finding: &Diagnostic,
        binding: &FixBinding,
        sources: &Sources,
    ) -> Result<Option<Ask>, String> {
        let engine = self.engine;
        let FixedBy::Fixer(id) = &binding.fixer else {
            return Err("a fix by a model is made by an agent that edits".to_owned());
        };
        let fixer = engine
            .registry
            .fixer(id)
            .ok_or_else(|| format!("fixer `{id}` is not registered"))?;
        let language = outcome
            .project
            .file(&finding.file)
            .map_or("", |f| f.lang.as_str());
        if let Some(gap) = self.overlay_gap(language) {
            return Err(gap);
        }
        if let Some(missing) = fixer
            .manifest()
            .requires
            .iter()
            .find(|c| !engine.provides(language, **c))
        {
            return Err(format!(
                "language `{language}` lacks the capability `{missing}` the fix needs"
            ));
        }
        sources.text(&finding.file)?;
        Ok(Some(self.asked(outcome, finding, None)))
    }

    /// What a fix by a command is asked, or why it cannot be; `None` for a
    /// program that is not a filter, which the graph cannot stand for.
    fn ask_command(
        &self,
        outcome: &Outcome,
        finding: &Diagnostic,
        fix: &super::CommandFix,
        sources: &Sources,
    ) -> Result<Option<Ask>, String> {
        let language = outcome
            .project
            .file(&finding.file)
            .map_or("", |f| f.lang.as_str());
        let program = fix.programs.of(language);
        if !program.is_some_and(is_filter) {
            return Ok(None);
        }
        let program = self.program_of(language, fix)?;
        sources.text(&finding.file)?;
        Ok(Some(self.asked(outcome, finding, Some(program.clone()))))
    }

    fn asked(&self, outcome: &Outcome, finding: &Diagnostic, program: Option<Program>) -> Ask {
        Ask {
            finding: finding.clone(),
            facts: outcome
                .facts
                .get(&finding.fingerprint)
                .cloned()
                .unwrap_or_default(),
            options: outcome
                .options
                .get(&finding.fingerprint)
                .cloned()
                .unwrap_or_default(),
            program,
        }
    }

    /// The fixes the graph holds for `asks`, in order, made over `texts`
    /// where they are not held. The fixes by a command are pulled first, and
    /// their time is the time of the commands.
    fn pull(
        &self,
        outcome: &Outcome,
        texts: &Overlays,
        asks: &[Ask],
    ) -> Result<Vec<Result<Arc<Fixed>, String>>, Error> {
        let mut guard = self.engine.lock_graph();
        let graph = guard.as_mut().ok_or_else(not_open)?;
        // The nodes read the texts of the graph: they are those of the round.
        self.engine
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_overlays(graph, texts)?;
        let rules: BTreeSet<&str> = asks.iter().map(|a| a.finding.rule_id.as_str()).collect();
        for rule in rules {
            if let Some(fixing) = self.fixing(rule) {
                graph.set_input(&FIXING, &rule.to_owned(), fixing, Durability::Stable)?;
            }
        }
        let programs: Vec<String> = asks
            .iter()
            .filter_map(|a| a.program.as_ref()?.command.first().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for (tool, identity) in tools::identities(
            graph,
            &self.engine.ws.root,
            &outcome.project.files,
            &programs,
        )? {
            graph.set_input(
                &lighthouse_plugin::TOOL,
                &tool,
                identity,
                Durability::Stable,
            )?;
        }
        let graph: &Graph = graph;
        let mut fixed: Vec<Option<Result<Arc<Fixed>, String>>> = vec![None; asks.len()];
        for commands in [true, false] {
            let at: Vec<usize> = (0..asks.len())
                .filter(|&n| asks[n].program.is_some() == commands)
                .collect();
            let keys: Vec<(String, String)> = at.iter().map(|&n| asks[n].key()).collect();
            let started = Instant::now();
            let pulled = crate::engine::in_pool(|| {
                graph.par_map(keys, |graph, key| {
                    graph
                        .query(&FIX, key)
                        .map_err(|e| format!("the fixer failed: {e}"))
                })
            });
            if commands {
                self.command_time
                    .set(self.command_time.get() + started.elapsed());
            }
            for (n, made) in at.into_iter().zip(pulled) {
                fixed[n] = Some(made);
            }
        }
        Ok(fixed.into_iter().flatten().collect())
    }

    /// How the fix of `rule` is declared to the graph.
    fn fixing(&self, rule: &str) -> Option<Fixing> {
        let name = self.run.fixer.as_deref().unwrap_or(rule);
        let binding = self.binding(rule)?;
        let command = self.plan.command(name);
        let mut hasher = hash::Hasher::new();
        hasher.update(self.engine.setting());
        hasher.update(self.plan.revision(name));
        Some(Fixing {
            fixer: match (&binding.fixer, command) {
                (FixedBy::Fixer(id), None) => id.clone(),
                _ => String::new(),
            },
            scope: command.map_or_else(|| self.plan.scope(name), |c| c.scope),
            description: command.map(|c| c.description.clone()).unwrap_or_default(),
            requirement: binding.decision.requirement.clone(),
            context: binding.decision.context.clone(),
            revision: hasher.hex(),
        })
    }

    /// The proposal a fix is, else why there is none.
    pub(super) fn assembled(
        &self,
        finding: &Diagnostic,
        binding: &FixBinding,
        made: &Made,
    ) -> Result<Proposal, String> {
        let safety = self.plan.safety(&finding.rule_id, made.safety, binding.cap);
        if safety == Safety::Unsafe && self.applies_previews() {
            return Err(NOT_KEPT.to_owned());
        }
        Ok(Proposal {
            fingerprint: finding.fingerprint.clone(),
            rule_id: finding.rule_id.clone(),
            file: finding.file.clone(),
            line: finding.span.start.line,
            fixer: binding.fixer.clone(),
            description: made.description.clone(),
            safety,
            priority: self.plan.priority(&finding.rule_id),
            edits: made.edits.clone(),
            files: made.bases.keys().cloned().collect(),
            aim: Aim::Finding,
            chain: 0,
            bases: made.bases.clone(),
            batch: vec![finding.fingerprint.clone()],
            moves_only: made.moves_only,
        })
    }
}

impl Proposer<'_> {
    /// Whether the run applies previews: it names findings and asks for
    /// `unsafe` fixes while the project does not write them unasked, where an
    /// `unsafe` fix is written only as it was shown. With `[fix] unsafe` the
    /// same words write a fix as it is made.
    pub(super) fn applies_previews(&self) -> bool {
        !self.run.fingerprints.is_empty()
            && self.run.unsafe_fixes == Some(true)
            && !self.engine.config.fix().unsafe_fixes
    }

    /// What applying previews makes of `finding`, if the run does: its kept
    /// preview, else why it has none. A fix that was never shown and may be
    /// unsafe is not made, and `None` leaves the finding to be fixed as ever.
    pub(super) fn applied_preview(
        &self,
        finding: &Diagnostic,
        binding: &FixBinding,
        sources: &Sources,
    ) -> Option<Result<Proposal, (Class, String)>> {
        if !self.applies_previews() {
            return None;
        }
        match self.kept(&finding.fingerprint, sources) {
            Ok(Some(proposal)) => Some(Ok(proposal)),
            Err(reason) => Some(Err((Class::Conflict, reason))),
            Ok(None) if binding.fixer == FixedBy::Model || binding.cap == Safety::Unsafe => {
                Some(Err((Class::Unanswered, NOT_KEPT.to_owned())))
            }
            Ok(None) => None,
        }
    }

    /// Keeps `proposal`, which the run withholds, for an application.
    pub(super) fn keep(&self, proposal: &Proposal) {
        if let (true, Ok(shown)) = (self.keeping, serde_json::to_string(proposal)) {
            self.pending
                .borrow_mut()
                .push((proposal.fingerprint.clone(), shown));
        }
    }

    /// The preview kept for the finding `fingerprint`: `None` when none was
    /// made or the graph keeps nothing, and why not when a file it changes is
    /// not as it was verified over.
    pub(super) fn kept(
        &self,
        fingerprint: &Fingerprint,
        sources: &Sources,
    ) -> Result<Option<Proposal>, String> {
        let shown = self.keeping.then(|| {
            let guard = self.engine.lock_graph();
            let graph = guard.as_ref()?;
            graph
                .input(&PREVIEW, &fingerprint.as_str().to_owned())
                .ok()?
        });
        let Some(proposal) = shown
            .flatten()
            .and_then(|shown| serde_json::from_str::<Proposal>(&shown).ok())
        else {
            return Ok(None);
        };
        let stale = proposal.bases.iter().find(|(file, base)| {
            !sources
                .text(file)
                .is_ok_and(|text| hash::sha256(text.as_str()) == **base)
        });
        match stale {
            Some((file, _)) => Err(format!(
                "`{}` is not as it was when the preview was made: apply the safe fixes first, or fix again for a new preview",
                file.display()
            )),
            None => Ok(Some(proposal)),
        }
    }

    /// Writes what the run kept, and the graph with it; why it could not, if
    /// it could not.
    pub(super) fn finish_kept(&self) -> Option<String> {
        let mut guard = self.engine.lock_graph();
        let graph = guard.as_mut()?;
        let kept = self.pending.take();
        let written = kept.into_iter().try_for_each(|(fingerprint, shown)| {
            graph.set_input(
                &PREVIEW,
                &fingerprint.as_str().to_owned(),
                shown,
                Durability::Stable,
            )
        });
        written
            .and_then(|()| graph.commit())
            .err()
            .map(|e| format!("the previews were not kept: {e}"))
    }
}

/// Where the fix of a finding is made.
enum Slot {
    /// In the graph: the position of its ask.
    Asked(usize),
    /// Without it, or not at all.
    Done(Box<Result<Proposal, String>>),
}

/// The finding left without a fix, and why.
fn declining(finding: &Diagnostic, reason: String) -> DeclinedFix {
    DeclinedFix {
        fingerprint: finding.fingerprint.clone(),
        rule_id: finding.rule_id.clone(),
        file: finding.file.clone(),
        line: finding.span.start.line,
        reason,
    }
}

fn not_open() -> Error {
    Error::Graph(lighthouse_graph::Error::Encode(
        "the graph is not open".to_owned(),
    ))
}
