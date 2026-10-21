//! Fix previews: what the fix of a finding changes. A preview is made by the
//! fix run itself, over the state the rounds leave, and by nothing else: a
//! check computes no fix. SARIF carries them as `fixes`.

use std::path::Path;

use lighthouse_model::{LineIndex, Position, Preview, PreviewEdit, PreviewFile, utf16_position};
use similar::TextDiff;

use super::{edits::apply, propose::Proposal};

/// Lines of unchanged text around a change in the hunks of a preview.
const CONTEXT: usize = 1;

/// What `proposal` changes, the text of its files being what `base` gives;
/// `None` when an edit does not apply or the proposal changes nothing.
pub(super) fn preview_of(
    proposal: &Proposal,
    base: impl Fn(&Path) -> Option<String>,
) -> Option<Preview> {
    Some(Preview {
        fingerprint: proposal.fingerprint.clone(),
        description: proposal.description.clone(),
        safety: proposal.safety,
        files: preview_files(proposal, base)?,
    })
}

/// What `proposal` changes in each of its files.
fn preview_files(
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use lighthouse_model::{Fingerprint, Safety};

    use super::*;
    use crate::fix::{
        FixedBy,
        edits::TextEdit,
        propose::{Aim, Proposal},
    };

    fn proposal(edits: Vec<TextEdit>) -> Proposal {
        Proposal {
            fingerprint: Fingerprint::new("r", "a.txt", "k"),
            rule_id: "r".to_owned(),
            file: "a.txt".into(),
            line: 1,
            fixer: FixedBy::Fixer("tidy".to_owned()),
            description: "tidy".to_owned(),
            safety: Safety::Safe,
            priority: 0,
            edits,
            files: vec!["a.txt".into()],
            aim: Aim::Finding,
            chain: 0,
            bases: BTreeMap::new(),
            batch: Vec::new(),
        }
    }

    fn replace(range: std::ops::Range<usize>, text: &str) -> TextEdit {
        TextEdit {
            file: "a.txt".into(),
            range,
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_fix_that_changes_nothing_has_no_preview_and_one_that_changes_the_text_has() {
        let base = |_: &Path| Some("one \n".to_owned());

        assert!(preview_of(&proposal(vec![replace(0..3, "one")]), base).is_none());
        let tidy = preview_of(&proposal(vec![replace(0..4, "one")]), base).unwrap();
        assert_eq!(tidy.files[0].hunks, "@@ -1 +1 @@\n-one \n+one");
    }
}
