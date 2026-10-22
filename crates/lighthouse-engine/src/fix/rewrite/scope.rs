//! The scopes of a fix, checked on what it left: under `symbol`, its changes to
//! a file must lie inside the extents of the findings it was asked about; under
//! `references`, in the files that refer to what it changed. The check is by
//! whole lines, so that indentation and the end of a line belong to the lines
//! they are on.

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    path::{Path, PathBuf},
};

use lighthouse_model::{
    Span,
    ucm::{Project, Symbol},
};
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

/// The files the scope `references` lets a change reach: `file`, and every
/// file that holds a reference site (a caller, a use, a site such as an
/// import) of a symbol that the change makes in a file already reached, again
/// and again, so that a signature changes with its callers and a caller whose
/// own signature changes, with its callers. `changes` holds the old and the
/// new text of each file the change makes.
pub(in crate::fix) fn reached(
    project: &Project,
    file: &Path,
    changes: &BTreeMap<PathBuf, (String, String)>,
) -> BTreeSet<PathBuf> {
    let mut reached = BTreeSet::from([file.to_owned()]);
    let mut pending = vec![file.to_owned()];
    while let Some(path) = pending.pop() {
        let Some((old, new)) = changes.get(&path) else {
            continue;
        };
        for symbol in changed_symbols(project, &path, old, new) {
            for holder in holders(project, symbol) {
                if reached.insert(holder.clone()) {
                    pending.push(holder);
                }
            }
        }
    }
    reached
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

/// The symbols of `path` whose lines change from `old` to `new`.
fn changed_symbols<'a>(project: &'a Project, path: &Path, old: &str, new: &str) -> Vec<&'a Symbol> {
    let changed: Vec<Range<usize>> = TextDiff::from_lines(old, new)
        .ops()
        .iter()
        .filter(|op| op.tag() != DiffTag::Equal)
        .map(|op| {
            let range = op.old_range();
            // An insertion belongs to the line it is at.
            range.start..range.end.max(range.start + 1)
        })
        .collect();
    project
        .symbols_in(path)
        .filter(|symbol| {
            let (first, end) = lines_of(&symbol.extent.unwrap_or(symbol.span));
            changed
                .iter()
                .any(|lines| lines.start < end.max(first + 1) && first < lines.end)
        })
        .collect()
}

/// The files that refer to `symbol`: those of its callers, of the symbols that
/// refer to it, and of its sites, which also name the places outside any
/// function.
fn holders(project: &Project, symbol: &Symbol) -> BTreeSet<PathBuf> {
    project
        .callers(&symbol.id)
        .iter()
        .chain(project.references(&symbol.id))
        .filter_map(|id| Some(project.symbol(id)?.file.clone()))
        .chain(
            project
                .sites(&symbol.id)
                .iter()
                .map(|site| site.file.clone()),
        )
        .collect()
}
