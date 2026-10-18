//! Fix previews: what the fix of a finding changes. A preview is made by the
//! fix run itself, over the state the rounds leave, and by nothing else: a
//! check computes no fix. SARIF carries them as `fixes`.

use std::path::Path;

use lighthouse_model::{LineIndex, Position, PreviewEdit, PreviewFile, utf16_position};
use similar::TextDiff;

use super::{edits::apply, propose::Proposal};

/// Lines of unchanged text around a change in the hunks of a preview.
const CONTEXT: usize = 1;

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
    let hunks = TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(CONTEXT)
        .to_string()
        .trim_end_matches('\n')
        .to_owned();
    let edits = edits
        .into_iter()
        .map(|(start, end, text)| PreviewEdit {
            start: utf16_position(before, start),
            end: utf16_position(before, end),
            text,
        })
        .collect();
    PreviewFile { path, hunks, edits }
}
