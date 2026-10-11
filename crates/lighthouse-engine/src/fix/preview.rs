//! Fix previews: what the fix of a finding would change, computed in memory.
//! A preview is the fixer's proposal lowered to text edits and applied to the
//! text on disk, nothing more: it is not formatted, not re-checked and not
//! written. Applying a fix goes through the verified path of a fix run.

use std::path::PathBuf;

use lighthouse_model::{Capability, Diagnostic, LineIndex, Preview, PreviewFile, Safety};

use super::{
    FixPlan, FixRun,
    edits::apply,
    lower::{Lowerer, Sources},
    run::{Orchestrator, provides},
};
use crate::{Engine, Outcome, Overlays};

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
    ) -> Vec<Preview> {
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
                let mut changed = Vec::new();
                for file in files {
                    let base = sources.text(&file).ok()?;
                    let mine: Vec<_> = proposal.edits.iter().filter(|e| e.file == file).collect();
                    let after = apply(&base, &mine).ok()?;
                    if after == *base {
                        continue;
                    }
                    let index = LineIndex::new(&base);
                    let edits = mine.iter().map(|e| {
                        (
                            index.position(e.range.start),
                            index.position(e.range.end),
                            e.text.clone(),
                        )
                    });
                    changed.push(PreviewFile::new(
                        file.to_string_lossy().replace('\\', "/"),
                        &base,
                        &after,
                        edits,
                    ));
                }
                if changed.is_empty() {
                    return None;
                }
                let needs_unsafe = proposal.safety == Safety::Suggested
                    || orchestrator.ineligible(binding).is_some();
                Some(Preview {
                    fingerprint: finding.fingerprint.clone(),
                    description: proposal.description,
                    safety: if needs_unsafe {
                        Safety::Suggested
                    } else {
                        Safety::Safe
                    },
                    files: changed,
                })
            })
            .collect()
    }
}
