//! Proposing the fixes of an agent that edits. The findings given to a model
//! are grouped by file, and the agent is run once for each file, one run
//! after another in the order of the paths: they form a chain. Each run starts
//! from the project with every edit the runs before it were accepted with, so
//! that whole-file results never overwrite each other, and its edit is
//! accepted only over the texts it found (see `conflict`). The files the chain
//! changed are then grouped: runs that changed a file in common share one
//! proposal, whose edit is the final text of each of their files, and the
//! checks say which findings it fixed.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use lighthouse_model::{Diagnostic, LineIndex, Position, Safety, Span, ucm::EditOp};
use lighthouse_spec::FixScope;

use super::{
    FixBinding, FixedBy,
    edits::{TextEdit, self_overlap},
    lower::{Lowerer, Sources},
    model::{Class, Event, MAX_FINDINGS, ModelFixer, World, choose_extent},
    propose::{Aim, Proposal, Proposer},
};
use crate::{Error, Overlays, Run, engine::Scopes};

/// A finding given to a model, and the fix that is asked of it.
type Asked<'o> = (&'o Diagnostic, FixBinding);

/// What a run of the chain left: the run, and the new texts of the files it
/// changed.
type Ran<'o> = (Accepted<'o>, Overlays);

/// The findings a run declines, and why.
type Failed<'o> = (Vec<&'o Diagnostic>, String);

/// A run of the chain that was accepted: the findings it was for, who made
/// the edit and the files it changed.
struct Accepted<'o> {
    findings: Vec<&'o Diagnostic>,
    model: String,
    files: Vec<PathBuf>,
}

impl Proposer<'_> {
    /// The proposals for the findings `found`, from the chain of runs over
    /// their files; the findings that get none are declined with the reason.
    pub(super) fn propose_edits<'o>(
        &self,
        outcome: &Run,
        texts: &Overlays,
        (lowerer, sources, scope): (&Lowerer, &Sources, &Scopes),
        found: Vec<Asked<'o>>,
        decline: &mut dyn FnMut(&Diagnostic, String),
    ) -> Result<Vec<Proposal>, Error> {
        let Some(models) = self.models.as_ref() else {
            return Ok(Vec::new());
        };
        let root = &self.engine.ws.root;
        let mut standing = texts.clone();
        let mut chain = Vec::new();
        for (file, mut group) in by_file(found) {
            let rest = group.split_off(group.len().min(MAX_FINDINGS));
            for (finding, _) in &rest {
                decline(
                    finding,
                    format!(
                        "left for the next run: an agent is given {MAX_FINDINGS} findings of a file at most"
                    ),
                );
            }
            let world = World::new(root, outcome, &standing, scope);
            match self.run_on(models, &world, &file, &group, sources, decline) {
                Ok(Some((accepted, edited))) => {
                    standing.extend(edited);
                    chain.push(accepted);
                }
                Ok(None) => {}
                Err((findings, reason)) => {
                    for finding in findings {
                        decline(finding, reason.clone());
                    }
                }
            }
        }
        Ok(self.proposals(chain, &standing, (lowerer, sources), decline))
    }

    /// One run of the chain over the findings `group` of `file`: the run, and
    /// the new texts of the files it changed. `None` when no finding could be
    /// given to the agent. A failure names the findings it declines.
    fn run_on<'o>(
        &self,
        models: &ModelFixer<'_>,
        world: &World<'_>,
        file: &Path,
        group: &[Asked<'o>],
        sources: &Sources,
        decline: &mut dyn FnMut(&Diagnostic, String),
    ) -> Result<Option<Ran<'o>>, Failed<'o>> {
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
        for path in edit.texts.keys() {
            if let Some(gap) = self.overlay_gap(&language_of(world, path)) {
                return Err(fail(gap));
            }
        }
        let files = edit.texts.keys().cloned().collect();
        Ok(Some((
            Accepted {
                findings: given.clone(),
                model: edit.model,
                files,
            },
            edit.texts,
        )))
    }

    /// The decisions whose safe fixes the project applies to what an agent
    /// leaves in `file`, which the agent is told.
    fn tools(&self, world: &World<'_>, file: &Path) -> Vec<String> {
        let language = language_of(world, file);
        self.plan
            .safe_fixes(Some(&language))
            .filter(|id| self.engine.active(id))
            .map(str::to_owned)
            .collect()
    }

    /// The proposals for the runs of the chain. Runs that changed a file in
    /// common are one group, with one proposal for each of their findings
    /// whose edit replaces every file of the group by the text it ended with.
    fn proposals(
        &self,
        chain: Vec<Accepted<'_>>,
        standing: &Overlays,
        (lowerer, sources): (&Lowerer, &Sources),
        decline: &mut dyn FnMut(&Diagnostic, String),
    ) -> Vec<Proposal> {
        let mut proposals = Vec::new();
        for (number, group) in grouped(chain).into_iter().enumerate() {
            let edits = match whole_files(&group, standing, (lowerer, sources)) {
                Ok(edits) => edits,
                Err(reason) => {
                    for finding in group.iter().flat_map(|run| &run.findings) {
                        decline(finding, reason.clone());
                    }
                    continue;
                }
            };
            let mut files: Vec<PathBuf> = edits.iter().map(|e| e.file.clone()).collect();
            files.sort();
            files.dedup();
            let model = &group[0].model;
            for finding in group.iter().flat_map(|run| &run.findings) {
                proposals.push(Proposal {
                    fingerprint: finding.fingerprint.clone(),
                    rule_id: finding.rule_id.clone(),
                    file: finding.file.clone(),
                    line: finding.span.start.line,
                    fixer: FixedBy::Model,
                    description: format!("model edit by `{model}`"),
                    safety: Safety::Suggested,
                    edits: edits.clone(),
                    files: files.clone(),
                    aim: Aim::File,
                    chain: number,
                });
            }
        }
        proposals
    }
}

/// The runs in groups: those that changed a file in common are together, in
/// the order they ran; the groups stand in the order of their last run.
fn grouped(chain: Vec<Accepted<'_>>) -> Vec<Vec<Accepted<'_>>> {
    let mut groups: Vec<Vec<Accepted<'_>>> = Vec::new();
    for run in chain {
        let mut together = vec![run];
        while let Some(at) = groups
            .iter()
            .position(|group| group.iter().any(|before| shares(before, &together)))
        {
            together.splice(0..0, groups.remove(at));
        }
        groups.push(together);
    }
    groups
}

/// Whether `run` changed a file that one of `others` changed.
fn shares(run: &Accepted<'_>, others: &[Accepted<'_>]) -> bool {
    others
        .iter()
        .any(|other| other.files.iter().any(|f| run.files.contains(f)))
}

/// The edit that replaces each file the runs of `group` changed by the text it
/// ended with, over the text it had at the start of the pass.
fn whole_files(
    group: &[Accepted<'_>],
    standing: &Overlays,
    (lowerer, sources): (&Lowerer, &Sources),
) -> Result<Vec<TextEdit>, String> {
    let mut edits = Vec::new();
    let mut files: Vec<&PathBuf> = group.iter().flat_map(|run| &run.files).collect();
    files.sort();
    files.dedup();
    for path in files {
        let Some(new) = standing.get(path) else {
            continue;
        };
        let text = sources.text(path)?;
        let whole = Span {
            start: Position { line: 1, col: 1 },
            end: LineIndex::new(&text).position(text.len()),
        };
        edits.extend(
            lowerer
                .lower(&EditOp::Replace {
                    file: path.clone(),
                    span: whole,
                    text: new.clone(),
                })
                .map_err(|why| format!("replace: {why}"))?,
        );
    }
    if edits.is_empty() {
        return Err("the fix changes nothing".to_owned());
    }
    if self_overlap(&edits) {
        return Err("the edits of the fix overlap each other".to_owned());
    }
    Ok(edits)
}

/// The language of `file`, empty when it is not indexed.
fn language_of(world: &World<'_>, file: &Path) -> String {
    world
        .outcome
        .project
        .file(file)
        .map_or_else(String::new, |f| f.lang.clone())
}

/// The findings by their file, in the order of the paths and then of their
/// lines.
fn by_file(found: Vec<Asked<'_>>) -> BTreeMap<PathBuf, Vec<Asked<'_>>> {
    let mut files: BTreeMap<PathBuf, Vec<Asked<'_>>> = BTreeMap::new();
    for asked in found {
        files.entry(asked.0.file.clone()).or_default().push(asked);
    }
    for group in files.values_mut() {
        group.sort_by_key(|(finding, _)| (finding.span.start.line, finding.span.start.col));
    }
    files
}
