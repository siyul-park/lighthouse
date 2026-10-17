//! The fix by an agent that edits. For the findings of one file, a program
//! runs in a scratch copy of the project with the findings as its prompt; the
//! files it left changed are the proposal, if they are the file it was asked
//! about and nothing else. What the agent says it did is never taken for a
//! verdict: the checks judge the result, as for any fix by a model.

use std::{path::Path, sync::PoisonError};

use lighthouse_model::Diagnostic;
use lighthouse_plugin::{EditRequest, FixDecision};
use serde::{Deserialize, Serialize};

use super::{
    Class, Event, ModelFixer,
    edit_prompt::{MAX_EXAMPLES, Target, prompt},
    prompt::examples_for,
    scratch::{Change, Changes, Scratch},
};
use crate::{
    Overlays, Run,
    cache::{Digest, KeyBuilder, digest_of},
    engine::{Member, Refusal, Scopes},
};

/// The findings of one file an agent is asked to fix, by the order of their
/// lines.
pub(in crate::fix) struct Batch<'a> {
    pub(in crate::fix) file: &'a Path,
    pub(in crate::fix) targets: Vec<Target<'a>>,
}

/// What the agent works over, which is also all it may read.
pub(in crate::fix) struct World<'a> {
    pub(in crate::fix) root: &'a Path,
    /// The check the findings come from.
    pub(in crate::fix) outcome: &'a Run,
    /// The texts that stand in for the files of the project.
    pub(in crate::fix) texts: &'a Overlays,
    /// Where the run fixes: the files it may change.
    pub(in crate::fix) scope: &'a Scopes,
    /// The files of the scope as they are, which an answer is kept under.
    digest: Digest,
}

impl<'a> World<'a> {
    pub(in crate::fix) fn new(
        root: &'a Path,
        outcome: &'a Run,
        texts: &'a Overlays,
        scope: &'a Scopes,
    ) -> Self {
        let mut files: Vec<(&Path, &str)> = outcome
            .project
            .files
            .iter()
            .filter(|file| scope.contains(&file.path))
            .map(|file| (file.path.as_path(), file.hash.as_str()))
            .collect();
        files.sort_unstable();
        Self {
            root,
            outcome,
            texts,
            scope,
            digest: digest_of(&files).unwrap_or_default(),
        }
    }
}

/// The new text of the file an agent was asked about, and who made it.
pub(in crate::fix) struct Edit {
    /// `<provider>/<model>`.
    pub(in crate::fix) model: String,
    pub(in crate::fix) text: String,
}

/// One run of an agent, as it is kept.
#[derive(Serialize, Deserialize)]
struct Attempt {
    changes: Changes,
    /// What the agent said, cut to a few kilobytes.
    said: String,
}

impl ModelFixer<'_> {
    /// The batch of the findings of `file` in `language`, each with the
    /// decision it is of.
    pub(in crate::fix) fn batch<'a>(
        &'a self,
        file: &'a Path,
        language: &str,
        findings: &[(&'a Diagnostic, &'a FixDecision)],
    ) -> Batch<'a> {
        let targets = findings
            .iter()
            .map(|&(finding, decision)| {
                let fix = self.fixes.get(&decision.id);
                Target {
                    finding,
                    decision,
                    instruction: fix.and_then(|f| f.instruction.as_deref()),
                    examples: fix.map_or_else(Vec::new, |f| {
                        examples_for(&f.examples, language, MAX_EXAMPLES)
                    }),
                }
            })
            .collect();
        Batch { file, targets }
    }

    /// The agent's edit for the findings of `batch`, or why there is none; the
    /// findings are counted either way.
    pub(in crate::fix) fn edit(
        &self,
        batch: &Batch<'_>,
        world: &World<'_>,
    ) -> Result<Edit, String> {
        self.count(batch, Event::Asked);
        let prompt = prompt(&batch.targets);
        let prompt_digest = digest_of(&prompt).unwrap_or_default();
        let key = |_: &Member<'_>, model: &str| key(model, &prompt_digest, &world.digest);
        let call = |member: &Member<'_>| run(member, &prompt, world);
        let (attempt, model) = match self.route.through(&key, &call) {
            Ok(found) => found,
            Err(Refusal::Budget) => {
                let reason = "the budget for models is spent".to_owned();
                return Err(self.decline_all(batch, Class::Unanswered, reason));
            }
            Err(Refusal::Failed(why)) => {
                let reason = format!("no model answered: {why}");
                return Err(self.decline_all(batch, Class::Unanswered, reason));
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
        match accept(attempt.changes, batch.file, world) {
            Ok(text) => Ok(Edit { model, text }),
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
    Ok(Attempt { changes, said })
}

/// The key of a run: the model at its version, the prompt, and the files the
/// agent may read.
fn key(model: &str, prompt: &Digest, files: &Digest) -> Digest {
    let mut key = KeyBuilder::new();
    key.part("edit-answer").part(model).part(prompt).part(files);
    key.finish()
}

/// The new text of `file` when it is the only file the agent changed, else
/// why the work is rejected whole.
fn accept(changes: Changes, file: &Path, world: &World<'_>) -> Result<String, (Class, String)> {
    if let Some(offence) = changes
        .iter()
        .find_map(|(path, change)| offence(path, change, file, world))
    {
        return Err((Class::OutOfScope, offence));
    }
    match changes.into_iter().next() {
        Some((_, Change::Text(text))) => Ok(text),
        _ => Err((Class::Unchanged, "the agent changed no file".to_owned())),
    }
}

/// What is wrong with the agent having done `change` to `path`, if anything:
/// it may change the text of `file` and nothing else.
fn offence(path: &Path, change: &Change, file: &Path, world: &World<'_>) -> Option<String> {
    let shown = path.display();
    if !world.scope.contains(path) {
        return Some(format!(
            "the agent changed `{shown}`, which is outside the files being fixed"
        ));
    }
    match change {
        Change::Created => Some(format!("the agent created `{shown}`")),
        Change::Deleted => Some(format!("the agent deleted `{shown}`")),
        Change::Binary => Some(format!("the agent changed `{shown}`, which is not text")),
        Change::Text(_) if world.outcome.project.file(path).is_none() => Some(format!(
            "the agent changed `{shown}`, which Lighthouse does not index"
        )),
        Change::Text(_) if path != file => Some(format!(
            "the agent changed `{shown}`, which none of the findings is in"
        )),
        Change::Text(_) => None,
    }
}
