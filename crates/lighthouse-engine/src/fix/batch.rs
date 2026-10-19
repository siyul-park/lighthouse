//! Proposing the fixes of an agent that edits. The findings given to a model
//! are grouped by file, the scope of their fix and the agent their decisions
//! are bound to, and the agent is run once for each group, one run after
//! another in the order of the paths, the narrower scope first, then of the
//! agents: they form a chain. Each run starts from the project
//! with every edit the runs before it were accepted with, so that whole-file
//! results never overwrite each other, and its edit is accepted only over
//! the texts it found (see `conflict`). Each run is a link of the chain with
//! proposals of its own, whose edit replaces each file it changed over the
//! text it started from, so that the orchestrator can apply, judge and undo
//! the runs one at a time, the newest first.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use lighthouse_model::{Diagnostic, Fingerprint, Safety};
use lighthouse_protocol::hash;
use lighthouse_spec::FixScope;

use super::{
    FixBinding, FixPlan, FixedBy,
    brief::safe_fixes,
    edits::TextEdit,
    lower::Sources,
    model::{Class, Event, MAX_FINDINGS, ModelFixer, choose_extent},
    propose::{Aim, Proposal, Proposer},
    rewrite::World,
};
use crate::{Error, Overlays, Run};

/// A finding given to a model, and the fix that is asked of it.
type Asked<'o> = (&'o Diagnostic, FixBinding);

/// The findings a run declines, and why.
type Failed<'o> = (Vec<&'o Diagnostic>, String);

/// The run each finding goes to: its file, the scope of its fix and the agent
/// its decision is bound to.
type RunKey = (PathBuf, FixScope, Vec<String>);

/// A run of the chain that was accepted: the findings it was for, who made
/// the edit, and each file it changed with the text it started from and the
/// one it left.
struct Accepted<'o> {
    findings: Vec<&'o Diagnostic>,
    model: String,
    files: BTreeMap<PathBuf, (String, String)>,
}

impl Proposer<'_> {
    /// The proposals for the findings `found`, from the chain of runs over
    /// their files; the findings that get none are declined with the reason.
    pub(super) fn propose_edits<'o>(
        &self,
        outcome: &Run,
        texts: &Overlays,
        sources: &Sources,
        found: Vec<Asked<'o>>,
        decline: &mut dyn FnMut(&Diagnostic, String),
    ) -> Result<Vec<Proposal>, Error> {
        let Some(models) = self.models.as_ref() else {
            return Ok(Vec::new());
        };
        let root = &self.engine.ws.root;
        let mut standing = texts.clone();
        let mut proposals = Vec::new();
        let mut accepted = 0;
        for ((file, ..), mut group) in by_run(found, models) {
            let rest = group.split_off(group.len().min(MAX_FINDINGS));
            for (finding, _) in &rest {
                decline(
                    finding,
                    format!(
                        "left for the next run: an agent is given {MAX_FINDINGS} findings of a file at most"
                    ),
                );
            }
            let world = World::new(root, outcome, &standing);
            let ran = self.run_on(models, &world, &file, &group, sources, decline);
            match ran {
                Ok(Some(run)) => {
                    standing.extend(
                        run.files
                            .iter()
                            .map(|(path, (_, after))| (path.clone(), after.clone())),
                    );
                    proposals.extend(run_proposals(self.plan, accepted, &run));
                    accepted += 1;
                }
                Ok(None) => {}
                Err((findings, reason)) => {
                    for finding in findings {
                        decline(finding, reason.clone());
                    }
                }
            }
        }
        Ok(proposals)
    }

    /// One run of the chain over the findings `group` of `file`. `None` when
    /// no finding could be given to the agent. A failure names the findings
    /// it declines.
    fn run_on<'o>(
        &self,
        models: &ModelFixer<'_>,
        world: &World<'_>,
        file: &Path,
        group: &[Asked<'o>],
        sources: &Sources,
        decline: &mut dyn FnMut(&Diagnostic, String),
    ) -> Result<Option<Accepted<'o>>, Failed<'o>> {
        let all: Vec<&'o Diagnostic> = group.iter().map(|(finding, _)| *finding).collect();
        let original = sources.text(file).map_err(|reason| (all.clone(), reason))?;
        let unchanged = world.text(file).is_some_and(|now| now == *original);
        let mut given = Vec::new();
        let mut targets = Vec::new();
        for (finding, binding) in group {
            let held = models.scope_of(&finding.rule_id) == FixScope::Symbol;
            let extent = match (held, unchanged) {
                (false, _) => Ok(None),
                (true, true) => choose_extent(&world.outcome.project, finding, &original)
                    .map(Some)
                    .map_err(|reason| (Class::NoExtent, reason)),
                (true, false) => Err((
                    Class::Conflict,
                    "an earlier edit of this pass changed the file, so its extent is no longer known"
                        .to_owned(),
                )),
            };
            match extent {
                Ok(span) => {
                    given.push(*finding);
                    targets.push((*finding, &binding.decision, span));
                }
                Err((class, reason)) => {
                    models.account.add(&finding.rule_id, Event::Declined(class));
                    decline(finding, reason);
                }
            }
        }
        if targets.is_empty() {
            return Ok(None);
        }
        let fail = |reason: String| (given.clone(), reason);
        if let Some(gap) = self.overlay_gap(&language_of(world, file)) {
            return Err(fail(gap));
        }
        let edit = models
            .edit(
                &models.batch(file, &targets, self.tools(world, file)),
                world,
            )
            .map_err(fail)?;
        let mut files = BTreeMap::new();
        for (path, after) in edit.texts {
            if let Some(gap) = self.overlay_gap(&language_of(world, &path)) {
                return Err(fail(gap));
            }
            let Some(before) = world.text(&path) else {
                return Err(fail(format!("`{}` cannot be read", path.display())));
            };
            files.insert(path, (before, after));
        }
        Ok(Some(Accepted {
            findings: given.clone(),
            model: edit.model,
            files,
        }))
    }

    /// The decisions whose safe fixes the project applies to what an agent
    /// leaves in `file`, which the agent is told.
    fn tools(&self, world: &World<'_>, file: &Path) -> Vec<String> {
        let language = language_of(world, file);
        safe_fixes(self.plan, Some(&language))
            .into_iter()
            .filter(|id| self.engine.active(id))
            .map(str::to_owned)
            .collect()
    }
}

/// The proposals of run number `number` of the chain: one for each of its
/// findings, all with the one edit that replaces each file the run changed,
/// over the text the run started from.
fn run_proposals(plan: &FixPlan, number: usize, run: &Accepted<'_>) -> Vec<Proposal> {
    let edits: Vec<TextEdit> = run
        .files
        .iter()
        .map(|(file, (before, after))| TextEdit {
            file: file.clone(),
            range: 0..before.len(),
            text: after.clone(),
        })
        .collect();
    let bases: BTreeMap<PathBuf, String> = run
        .files
        .iter()
        .map(|(file, (before, _))| (file.clone(), hash::sha256(before)))
        .collect();
    let files: Vec<PathBuf> = run.files.keys().cloned().collect();
    let batch: Vec<Fingerprint> = run
        .findings
        .iter()
        .map(|finding| finding.fingerprint.clone())
        .collect();
    run.findings
        .iter()
        .map(|finding| Proposal {
            fingerprint: finding.fingerprint.clone(),
            rule_id: finding.rule_id.clone(),
            file: finding.file.clone(),
            line: finding.span.start.line,
            fixer: FixedBy::Model,
            description: format!("model edit by `{}`", run.model),
            safety: plan.safety(&finding.rule_id, Safety::Unsafe, Safety::Unsafe),
            priority: plan.priority(&finding.rule_id),
            edits: edits.clone(),
            files: files.clone(),
            aim: Aim::File,
            chain: number,
            bases: bases.clone(),
            batch: batch.clone(),
        })
        .collect()
}

/// The language of `file`, empty when it is not indexed.
fn language_of(world: &World<'_>, file: &Path) -> String {
    world
        .outcome
        .project
        .file(file)
        .map_or_else(String::new, |f| f.lang.clone())
}

/// The findings by the run they go to, in the order of the paths, the
/// narrower scope first, then of the agents, and then of their lines.
fn by_run<'o>(found: Vec<Asked<'o>>, models: &ModelFixer<'_>) -> BTreeMap<RunKey, Vec<Asked<'o>>> {
    let mut runs: BTreeMap<RunKey, Vec<Asked<'o>>> = BTreeMap::new();
    for asked in found {
        let rule = &asked.0.rule_id;
        let key = (
            asked.0.file.clone(),
            models.scope_of(rule),
            models.agent_for(rule).to_vec(),
        );
        runs.entry(key).or_default().push(asked);
    }
    for group in runs.values_mut() {
        group.sort_by_key(|(finding, _)| (finding.span.start.line, finding.span.start.col));
    }
    runs
}
