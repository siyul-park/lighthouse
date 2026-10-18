//! The fix by an agent that edits. For the findings of one file, a program
//! runs in a scratch copy of the project with the brief as its prompt; the
//! files it left changed are the proposal, if they are within the scope of
//! the fix and nothing else changed. What the agent says it did is never taken
//! for a verdict: the checks judge the result, as for any fix by a model.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::PoisonError,
};

use lighthouse_model::{Diagnostic, Span, ucm::File};
use lighthouse_plugin::{EditRequest, FixDecision};
use lighthouse_protocol::hash;
use lighthouse_spec::FixScope;
use serde::{Deserialize, Serialize};

use super::{
    Class, Event, ModelFixer,
    scope::first_outside,
    scratch::{Change, Changes, Scratch},
};
use crate::{
    Overlays, Run,
    cache::{Digest, KeyBuilder, digest_of},
    engine::{Member, Refusal, Scopes},
    fix::brief::{Target, brief},
};

/// The findings of one file an agent is asked to fix, by the order of their
/// lines.
pub(in crate::fix) struct Batch<'a> {
    pub(in crate::fix) file: &'a Path,
    targets: Vec<Target<'a>>,
    /// The agent the run asks, in the order it is tried: the binding the
    /// findings share.
    binding: Vec<String>,
    /// The decisions whose safe fixes the project applies to the result.
    tools: Vec<String>,
    /// The widest change allowed: the narrowest scope of the findings.
    scope: FixScope,
    /// Where the agent may change `file` under the scope `symbol`: the extent
    /// of each finding.
    extents: Vec<Span>,
}

/// What the agent works over, which is also all it may read.
pub(in crate::fix) struct World<'a> {
    pub(in crate::fix) root: &'a Path,
    /// The check the findings come from.
    pub(in crate::fix) outcome: &'a Run,
    /// The texts that stand in for the files of the project: those of the
    /// rounds before, and every edit already accepted in this pass.
    pub(in crate::fix) texts: &'a Overlays,
    /// Where the run fixes: the files it may change.
    pub(in crate::fix) scope: &'a Scopes,
    /// The files of the scope as they stand now, which an answer is kept
    /// under: an edit made over other texts is another answer.
    digest: Digest,
}

impl<'a> World<'a> {
    pub(in crate::fix) fn new(
        root: &'a Path,
        outcome: &'a Run,
        texts: &'a Overlays,
        scope: &'a Scopes,
    ) -> Result<Self, String> {
        let mut files: Vec<(&Path, String)> = outcome
            .project
            .files
            .iter()
            .filter(|file| scope.contains(&file.path))
            .map(|file| (file.path.as_path(), stand(texts, file)))
            .collect();
        files.sort_unstable();
        let digest = digest_of(&files)
            .map_err(|e| format!("the files of the project could not be digested: {e}"))?;
        Ok(Self {
            root,
            outcome,
            texts,
            scope,
            digest,
        })
    }

    /// The text `path` has before the agent runs.
    pub(in crate::fix) fn text(&self, path: &Path) -> Option<String> {
        match self.texts.get(path) {
            Some(text) => Some(text.clone()),
            None => fs::read_to_string(self.root.join(path)).ok(),
        }
    }
}

/// The new texts of the files an agent changed, by path, and who made them.
pub(in crate::fix) struct Edit {
    /// `<provider>/<model>`.
    pub(in crate::fix) model: String,
    pub(in crate::fix) texts: BTreeMap<PathBuf, String>,
}

/// One run of an agent, as it is kept.
#[derive(Serialize, Deserialize)]
struct Attempt {
    changes: Changes,
    /// The digest of each file it changed as the run found it.
    bases: BTreeMap<PathBuf, String>,
    /// What the agent said, cut to a few kilobytes.
    said: String,
}

impl ModelFixer<'_> {
    /// The batch of the findings of `file`, each with the decision it is of
    /// and, for the scope `symbol`, its extent.
    pub(in crate::fix) fn batch<'a>(
        &'a self,
        file: &'a Path,
        findings: &[(&'a Diagnostic, &'a FixDecision, Option<Span>)],
        tools: Vec<String>,
    ) -> Batch<'a> {
        let scope = findings
            .iter()
            .map(|(_, decision, _)| self.scope_of(&decision.id))
            .min()
            .unwrap_or_default();
        let targets = findings
            .iter()
            .map(|&(finding, decision, _)| {
                let fix = self.fixes.get(&decision.id);
                Target {
                    decision,
                    instruction: fix.and_then(|f| f.instruction.as_deref()),
                    examples: fix.map_or_else(Vec::new, |f| f.examples.clone()),
                    scope,
                    message: &finding.message,
                    evidence: &finding.evidence,
                    place: format!(
                        "{}:{}:{}",
                        finding.file.display(),
                        finding.span.start.line,
                        finding.span.start.col
                    ),
                }
            })
            .collect();
        let binding = findings
            .first()
            .map(|(_, decision, _)| self.edit_for(&decision.id).to_vec())
            .unwrap_or_default();
        Batch {
            file,
            binding,
            scope,
            extents: findings.iter().filter_map(|&(_, _, span)| span).collect(),
            targets,
            tools,
        }
    }

    /// The agent's edit for the findings of `batch`, or why there is none; the
    /// findings are counted either way.
    pub(in crate::fix) fn edit(
        &self,
        batch: &Batch<'_>,
        world: &World<'_>,
    ) -> Result<Edit, String> {
        self.count(batch, Event::Asked);
        let tools: Vec<&str> = batch.tools.iter().map(String::as_str).collect();
        let prompt = brief(&batch.targets, &tools);
        let prompt_digest = match digest_of(&prompt) {
            Ok(digest) => digest,
            Err(e) => {
                let reason = format!("the brief could not be digested: {e}");
                return Err(self.decline_all(batch, Class::Unanswered, reason));
            }
        };
        let key = |_: &Member<'_>, model: &str| key(model, &prompt_digest, &world.digest);
        let call = |member: &Member<'_>| run(member, &prompt, world);
        let (attempt, model) = match self.route.through(&batch.binding, &key, &call) {
            Ok(found) => found,
            Err(Refusal::Budget) => {
                let reason = "the budget for models is spent".to_owned();
                return Err(self.decline_all(batch, Class::Unanswered, reason));
            }
            Err(Refusal::Failed(why)) => {
                let reason = format!("no model answered: {why}");
                return Err(self.decline_all(batch, Class::Unanswered, reason));
            }
            Err(Refusal::Unkept) => {
                let reason = "no kept answer matches the files as they are now: they changed since the preview, or it was never made (run `check --fix` again for a new one)".to_owned();
                return Err(self.decline_all(batch, Class::Conflict, reason));
            }
        };
        self.count(batch, Event::Answered);
        if !attempt.said.is_empty() {
            let said = format!("{model} on {}: {}", batch.file.display(), attempt.said);
            self.said
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(said);
        }
        if let Some(reason) = conflict(&attempt.bases, world) {
            return Err(self.decline_all(batch, Class::Conflict, reason));
        }
        match accept(attempt.changes, batch, world) {
            Ok(texts) => Ok(Edit { model, texts }),
            Err((class, reason)) => Err(self.decline_all(batch, class, reason)),
        }
    }

    fn count(&self, batch: &Batch<'_>, event: Event) {
        for target in &batch.targets {
            self.account.add(&target.decision.id, event);
        }
    }

    /// Counts every finding of `batch` as declined, and returns the reason.
    fn decline_all(&self, batch: &Batch<'_>, class: Class, reason: String) -> String {
        self.count(batch, Event::Declined(class));
        reason
    }
}

/// Why the edit cannot be accepted, if a file it changed no longer has the
/// digest it was made over (compare and swap). Edits are never merged: the
/// findings go to review and a later run starts again from what stands.
pub(in crate::fix) fn conflict(
    bases: &BTreeMap<PathBuf, String>,
    world: &World<'_>,
) -> Option<String> {
    bases.iter().find_map(|(path, base)| {
        let now = world.text(path).map(|text| hash::sha256(&text));
        (now.as_ref() != Some(base)).then(|| {
            format!(
                "`{}` is not as the agent found it: an earlier edit of this pass changed it",
                path.display()
            )
        })
    })
}

/// The digest of the file as it stands: the text that stands in for it, else
/// the hash of the file on disk.
fn stand(texts: &Overlays, file: &File) -> String {
    texts
        .get(&file.path)
        .map_or_else(|| file.hash.clone(), hash::sha256)
}

/// What `member` leaves of the scratch copy of the project once it has been
/// asked `prompt` in it. The copy is deleted on the way out, whatever happens.
fn run(member: &Member<'_>, prompt: &str, world: &World<'_>) -> Result<Attempt, String> {
    let scratch = Scratch::new(world.root, world.texts)
        .map_err(|e| format!("the scratch copy of the project failed: {e}"))?;
    let said = member
        .client
        .edit(&EditRequest {
            prompt,
            dir: scratch.path(),
        })
        .map_err(|e| e.to_string())?;
    let changes = scratch
        .changes(world.root, world.texts)
        .map_err(|e| format!("the scratch copy of the project could not be read: {e}"))?;
    let mut bases = BTreeMap::new();
    for (path, change) in &changes {
        if *change == Change::Created {
            continue;
        }
        let found = world.text(path).ok_or_else(|| {
            format!(
                "`{}`, which the agent changed, cannot be read",
                path.display()
            )
        })?;
        bases.insert(path.clone(), hash::sha256(&found));
    }
    Ok(Attempt {
        changes,
        bases,
        said,
    })
}

/// The key of a run: the model at its version, the prompt, and the files the
/// agent may read.
fn key(model: &str, prompt: &Digest, files: &Digest) -> Digest {
    let mut key = KeyBuilder::new();
    key.part("edit-answer").part(model).part(prompt).part(files);
    key.finish()
}

/// The new texts of the files the agent changed when all of it is within the
/// scope of `batch`, else why the work is rejected whole.
fn accept(
    changes: Changes,
    batch: &Batch<'_>,
    world: &World<'_>,
) -> Result<BTreeMap<PathBuf, String>, (Class, String)> {
    if let Some(offence) = changes
        .iter()
        .find_map(|(path, change)| offence(path, change, batch, world))
    {
        return Err((Class::OutOfScope, offence));
    }
    let texts: BTreeMap<PathBuf, String> = changes
        .into_iter()
        .filter_map(|(path, change)| match change {
            Change::Text(text) => Some((path, text)),
            _ => None,
        })
        .collect();
    if texts.is_empty() {
        return Err((Class::Unchanged, "the agent changed no file".to_owned()));
    }
    Ok(texts)
}

/// What is wrong with the agent having done `change` to `path`, if anything:
/// it may change the indexed text files of the report scope, and under the
/// scope `symbol` only the extents of the findings in their file.
fn offence(path: &Path, change: &Change, batch: &Batch<'_>, world: &World<'_>) -> Option<String> {
    let shown = path.display();
    if !world.scope.contains(path) {
        return Some(format!(
            "the agent changed `{shown}`, which is outside the files being fixed"
        ));
    }
    let Change::Text(text) = change else {
        return Some(match change {
            Change::Created => format!("the agent created `{shown}`"),
            Change::Deleted => format!("the agent deleted `{shown}`"),
            _ => format!("the agent changed `{shown}`, which is not text"),
        });
    };
    if world.outcome.project.file(path).is_none() {
        return Some(format!(
            "the agent changed `{shown}`, which Lighthouse does not index"
        ));
    }
    if batch.scope == FixScope::Files {
        return None;
    }
    if path != batch.file {
        return Some(format!(
            "the agent changed `{shown}`, which none of the findings is in (scope `symbol`)"
        ));
    }
    let Some(old) = world.text(path) else {
        return Some(format!("`{shown}` cannot be read"));
    };
    let line = first_outside(&old, text, &batch.extents)?;
    Some(format!(
        "the agent changed line {line} of `{shown}`, outside the symbol of the findings (scope `symbol`)"
    ))
}
