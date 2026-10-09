//! Fix previews: what the fix of a finding would change, computed in memory.
//! A preview is the fixer's proposal lowered to text edits and applied to the
//! text on disk, nothing more: it is not formatted, not re-checked and not
//! written. Applying a fix goes through the verified path of a fix run.

use std::path::PathBuf;

use lighthouse_model::{Capability, Diagnostic, Fingerprint, LineIndex, Position, Safety};

use super::{
    FileChange, FixPlan, FixRun,
    edits::apply,
    lower::{Lowerer, Sources},
    run::{Orchestrator, provides},
};
use crate::{Engine, Outcome, Overlays};

/// One replacement of a preview: the span it deletes (empty for an insertion)
/// in the original text, and what takes its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewEdit {
    pub file: PathBuf,
    pub start: Position,
    pub end: Position,
    pub text: String,
}

/// The fix proposed for one finding, as it would change the files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixPreview {
    pub fingerprint: Fingerprint,
    pub description: String,
    /// `Suggested` when applying the fix needs unsafe fixes, whatever the
    /// fixer claims: a suggested proposal, a suggested cap or a rule that is
    /// not mechanical.
    pub safety: Safety,
    pub edits: Vec<PreviewEdit>,
    /// The files as they would be, sorted by path.
    pub changes: Vec<FileChange>,
}

impl Engine {
    /// The previews of the fixes of `findings`, those that propose a fix that
    /// applies cleanly and changes something. A finding without a fix, one
    /// whose fixer runs a program (it cannot run without trust) and one the
    /// fixer declines have none.
    pub fn preview_fixes(
        &self,
        plan: &FixPlan,
        outcome: &Outcome,
        findings: &[&Diagnostic],
    ) -> Vec<FixPreview> {
        let run = FixRun::default();
        let orchestrator = Orchestrator::for_preview(self, plan, &run);
        let overlays = Overlays::new();
        let sources = Sources::new(&self.ws.root, &outcome.project, &overlays);
        let complete = |language: &str| provides(self, language, Capability::CompleteReferences);
        let lowerer = Lowerer::new(&outcome.project, &sources, &complete);
        findings
            .iter()
            .filter_map(|finding| {
                let binding = plan.get(&finding.rule_id)?;
                if binding.unsupported.is_some() || plan.runs_command(&finding.rule_id) {
                    return None;
                }
                let proposal = orchestrator
                    .propose_one(outcome, finding, binding, &lowerer, &sources)
                    .ok()?;
                let mut files: Vec<PathBuf> =
                    proposal.edits.iter().map(|e| e.file.clone()).collect();
                files.sort();
                files.dedup();
                let mut changes = Vec::new();
                let mut edits = Vec::new();
                for file in files {
                    let base = sources.text(&file).ok()?;
                    let mine: Vec<_> = proposal.edits.iter().filter(|e| e.file == file).collect();
                    let after = apply(&base, &mine).ok()?;
                    if after == *base {
                        continue;
                    }
                    let index = LineIndex::new(&base);
                    edits.extend(mine.iter().map(|e| PreviewEdit {
                        file: file.clone(),
                        start: index.position(e.range.start),
                        end: index.position(e.range.end),
                        text: e.text.clone(),
                    }));
                    changes.push(FileChange {
                        path: file,
                        before: (*base).clone(),
                        after,
                    });
                }
                if changes.is_empty() {
                    return None;
                }
                let needs_unsafe = proposal.safety == Safety::Suggested
                    || orchestrator.ineligible(binding).is_some();
                Some(FixPreview {
                    fingerprint: finding.fingerprint.clone(),
                    description: proposal.description,
                    safety: if needs_unsafe {
                        Safety::Suggested
                    } else {
                        Safety::Safe
                    },
                    edits,
                    changes,
                })
            })
            .collect()
    }
}
