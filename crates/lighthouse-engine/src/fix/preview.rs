//! Fix previews: what the fix of a finding changes. A preview is made by the
//! fix run itself, over the state the rounds leave, so that a finding shows the
//! diff the run would write, or keep for a hand, and never another.

use std::path::Path;

use lighthouse_model::{
    Diagnostic, LineIndex, Position, Preview, PreviewEdit, PreviewFile, utf16_position,
};
use similar::{ChangeTag, TextDiff};

use super::{FixPlan, FixRun, edits::apply, propose::Proposal};
use crate::Engine;

/// Lines of unchanged text around a change in the hunks of a preview.
const CONTEXT: usize = 1;

impl Engine {
    /// The previews of the fixes of `findings`: what a dry run of the fixes
    /// would change, one for each finding whose fix is accepted by the checks,
    /// whoever makes it but a model, which is not asked. A finding without a
    /// fix and one the fixer declines have none.
    pub fn preview_fixes(&self, plan: &FixPlan, findings: &[&Diagnostic]) -> Vec<Preview> {
        let fingerprints: Vec<String> = findings
            .iter()
            .filter(|finding| plan.get(&finding.rule_id).is_some())
            .map(|finding| finding.fingerprint.as_str().to_owned())
            .collect();
        if fingerprints.is_empty() {
            return Vec::new();
        }
        let run = FixRun {
            fingerprints,
            dry_run: true,
            no_models: true,
            ..FixRun::default()
        };
        let Ok(report) = self.fix(plan, &run) else {
            return Vec::new();
        };
        let applied = report.applied.into_iter().filter_map(|fix| fix.preview);
        applied
            .chain(report.previews.into_iter().map(|fix| fix.preview))
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
