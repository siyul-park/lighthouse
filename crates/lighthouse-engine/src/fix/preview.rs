//! Fix previews: what the fix of a finding would change, computed in memory.
//! A preview is the fixer's proposal lowered to text edits and applied to the
//! text on disk, nothing more: it is not formatted, not re-checked and not
//! written. Applying a fix goes through the verified path of a fix run.

use std::path::Path;

use lighthouse_model::{
    Diagnostic, LineIndex, Position, Preview, PreviewEdit, PreviewFile, ucm::Capability,
    utf16_position,
};
use similar::{ChangeTag, TextDiff};

use super::{
    FixPlan, FixRun, FixedBy,
    edits::apply,
    lower::{Lowerer, Sources},
    propose::{Proposal, Proposer},
};
use crate::{Engine, Overlays, Run};

/// Lines of unchanged text around a change in the hunks of a preview.
const CONTEXT: usize = 1;

impl Engine {
    /// The previews of the fixes of `findings`, those that propose a fix that
    /// applies cleanly and changes something. A finding without a fix, one
    /// whose fixer runs a program (it cannot run without trust) and one the
    /// fixer declines have none.
    pub fn preview_fixes(
        &self,
        plan: &FixPlan,
        outcome: &Run,
        findings: &[&Diagnostic],
    ) -> Vec<Preview> {
        let run = FixRun::default();
        let proposer = Proposer::new(self, plan, &run);
        let overlays = Overlays::new();
        let sources = Sources::new(&self.ws.root, &outcome.project, &overlays);
        let complete = |language: &str| self.provides(language, Capability::CompleteReferences);
        let lowerer = Lowerer::new(&outcome.project, &sources, &complete);
        findings
            .iter()
            .filter_map(|finding| {
                let binding = plan.get(&finding.rule_id)?;
                if binding.unsupported.is_some()
                    || binding.runs_command
                    || binding.fixer == FixedBy::Model
                {
                    return None;
                }
                let proposal = proposer
                    .propose_one(outcome, finding, binding, &lowerer, &sources)
                    .ok()?;
                let changed = preview_files(&proposal, |file| {
                    sources.text(file).ok().map(|text| (*text).clone())
                })?;
                Some(Preview {
                    fingerprint: finding.fingerprint.clone(),
                    description: proposal.description,
                    safety: proposal.safety,
                    files: changed,
                })
            })
            .collect()
    }
}

/// What `proposal` changes in each of its files, the text of which `base`
/// gives; `None` when an edit does not apply or the proposal changes nothing.
pub(super) fn preview_files(
    proposal: &Proposal,
    base: impl Fn(&Path) -> Option<String>,
) -> Option<Vec<PreviewFile>> {
    let mut changed = Vec::new();
    for file in &proposal.files {
        let base = base(file)?;
        let mine: Vec<_> = proposal.edits.iter().filter(|e| e.file == *file).collect();
        let after = apply(&base, &mine).ok()?;
        if after == base {
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
        changed.push(preview_file(
            file.to_string_lossy().replace('\\', "/"),
            &base,
            &after,
            edits,
        ));
    }
    (!changed.is_empty()).then_some(changed)
}

/// The change of one file from `before` to `after`, with `edits` as
/// (start, end, text) in byte columns of `before`.
fn preview_file(
    path: String,
    before: &str,
    after: &str,
    edits: impl IntoIterator<Item = (Position, Position, String)>,
) -> PreviewFile {
    let diff = TextDiff::from_lines(before, after);
    let hunks = diff
        .unified_diff()
        .context_radius(CONTEXT)
        .to_string()
        .trim_end_matches('\n')
        .to_owned();
    let changed = diff
        .iter_all_changes()
        .filter(|c| c.tag() != ChangeTag::Equal)
        .count();
    let edits = edits
        .into_iter()
        .map(|(start, end, text)| PreviewEdit {
            start: utf16_position(before, start),
            end: utf16_position(before, end),
            text,
        })
        .collect();
    PreviewFile {
        path,
        hunks,
        changed,
        edits,
    }
}
