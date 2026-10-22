//! The fix by an agent that edits. For the findings of one file, a program
//! runs in a scratch copy of the project with the brief as its prompt; the
//! files it left changed are the proposal, if they are within the scope of
//! the fix and nothing else changed. What the agent says it did is never taken
//! for a verdict: the checks judge the result, as for any fix by a model.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::PoisonError,
};

use lighthouse_model::{Diagnostic, Span, Unavailable};
use lighthouse_plugin::{EditRequest, Edited, FixDecision};
use lighthouse_protocol::ToolEvent;
use lighthouse_protocol::hash;
use lighthouse_resource::slashed;
use lighthouse_spec::FixScope;
use serde::{Deserialize, Serialize};

use super::{BatchTrace, Class, Event, ModelFixer};
use crate::{
    cache::{Digest, KeyBuilder, digest_of},
    engine::{Member, Refusal},
    fix::{
        brief::{Localized, Target, brief},
        context::localize,
        rewrite::{Bounds, Changes, Scratch, World, accept, collect},
    },
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
    /// What the code model knows around the findings, for the brief.
    localized: Localized,
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
    /// What it did, from its harness. Not kept: an answer kept from an
    /// earlier run did nothing in this one.
    #[serde(skip)]
    events: Vec<ToolEvent>,
    /// Why `events` is not all it did, when it is not. Not kept either.
    #[serde(skip)]
    incomplete: Option<String>,
}

impl ModelFixer<'_> {
    /// The batch of the findings of `file`, each with the decision it is of
    /// and, for the scope `symbol`, its extent.
    pub(in crate::fix) fn batch<'a>(
        &'a self,
        world: &World<'_>,
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
        let localized = localize(
            world,
            file,
            &findings
                .iter()
                .map(|&(finding, _, span)| (finding, span))
                .collect::<Vec<_>>(),
            scope,
            self.context,
        );
        let binding = findings
            .first()
            .map(|(_, decision, _)| self.agent_for(&decision.id).to_vec())
            .unwrap_or_default();
        Batch {
            file,
            binding,
            scope,
            extents: findings.iter().filter_map(|&(_, _, span)| span).collect(),
            localized,
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
        let prompt = brief(&batch.targets, &tools, Some(&batch.localized));
        let prompt_digest = match digest_of(&prompt) {
            Ok(digest) => digest,
            Err(e) => {
                let reason = format!("the brief could not be digested: {e}");
                return Err(self.decline_all(batch, Class::Unanswered, reason));
            }
        };
        let files = match world.digest() {
            Ok(digest) => digest,
            Err(reason) => return Err(self.decline_all(batch, Class::Unanswered, reason)),
        };
        let key = |_: &Member<'_>, model: &str| key(model, &prompt_digest, files);
        let call = |member: &Member<'_>| run(member, &prompt, world);
        let (attempt, model) = match self.route.through(&batch.binding, &key, &call) {
            Ok(found) => found,
            Err(Refusal::Budget) => {
                let reason = format!("left for review: {}", Unavailable::BudgetSpent);
                return Err(self.decline_all(batch, Class::Unanswered, reason));
            }
            Err(Refusal::Failed(why)) => {
                let reason = format!("no model answered: {why}");
                return Err(self.decline_all(batch, Class::Unanswered, reason));
            }
        };
        self.count(batch, Event::Answered);
        if !attempt.events.is_empty() || attempt.incomplete.is_some() {
            let trace = BatchTrace::of(
                batch.file.to_owned(),
                &batch.briefed(),
                &attempt.events,
                attempt.incomplete.clone(),
            );
            self.traces
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(trace);
        }
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
        let bounds = Bounds {
            who: "the agent",
            scope: batch.scope,
            file: batch.file,
            extents: &batch.extents,
        };
        match accept(attempt.changes, &bounds, world) {
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

impl Batch<'_> {
    /// The files the brief shows or names: the one the findings are in, and
    /// what the engine localized around them that fit the brief.
    fn briefed(&self) -> BTreeSet<String> {
        let mut briefed = self.localized.briefed.clone();
        briefed.insert(slashed(self.file));
        briefed
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

/// What `member` leaves of the scratch copy of the project once it has been
/// asked `prompt` in it. The copy is deleted on the way out, whatever happens.
fn run(member: &Member<'_>, prompt: &str, world: &World<'_>) -> Result<Attempt, String> {
    let scratch = Scratch::new(world.root, world.texts)
        .map_err(|e| format!("the scratch copy of the project failed: {e}"))?;
    let Edited {
        said,
        events,
        incomplete,
    } = member
        .client
        .edit(&EditRequest {
            prompt,
            dir: scratch.path(),
            model: "",
        })
        .map_err(|e| e.to_string())?;
    let (changes, bases) = collect(&scratch, world)?;
    Ok(Attempt {
        changes,
        bases,
        said,
        events,
        incomplete,
    })
}

/// The key of a run: the model at its version, the prompt, and the files the
/// agent may read.
fn key(model: &str, prompt: &Digest, files: &Digest) -> Digest {
    let mut key = KeyBuilder::new();
    key.part("edit-answer").part(model).part(prompt).part(files);
    key.finish()
}
