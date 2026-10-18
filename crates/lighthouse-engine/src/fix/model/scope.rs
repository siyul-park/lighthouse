//! The scope `symbol` of a fix by a model, checked on what the agent left: its
//! changes to a file must lie inside the extents of the findings it was asked
//! about. The check is by whole lines, so that indentation and the end of a
//! line belong to the lines they are on.

use lighthouse_model::Span;
use similar::{DiffTag, TextDiff};

/// The line, counted from 1 in `old`, of the first change to `old` that
/// makes `new` and lies outside every one of `extents`; `None` when all of
/// them are inside. An insertion at the edge of an extent is inside it.
pub(super) fn first_outside(old: &str, new: &str, extents: &[Span]) -> Option<usize> {
    let lines: Vec<(usize, usize)> = extents.iter().map(lines_of).collect();
    let covers = |line: usize| {
        lines
            .iter()
            .any(|&(first, end)| first <= line && line < end)
    };
    TextDiff::from_lines(old, new)
        .ops()
        .iter()
        .filter(|op| op.tag() != DiffTag::Equal)
        .map(|op| op.old_range())
        .find_map(|changed| {
            if changed.is_empty() {
                // An insertion belongs to the extent it is in or next to.
                let at = changed.start;
                let inside = lines.iter().any(|&(first, end)| first <= at && at <= end);
                return (!inside).then_some(at + 1);
            }
            changed
                .clone()
                .find(|&line| !covers(line))
                .map(|line| line + 1)
        })
}

/// The lines `extent` covers as a range of zero-based line numbers: the first
/// and one past the last. An extent that ends at the start of a line does not
/// cover it.
fn lines_of(extent: &Span) -> (usize, usize) {
    let first = extent.start.line.saturating_sub(1) as usize;
    let ends_before = extent.end.col == 1 && extent.end.line > extent.start.line;
    let end = extent.end.line.saturating_sub(u32::from(ends_before)) as usize;
    (first, end.max(first))
}
