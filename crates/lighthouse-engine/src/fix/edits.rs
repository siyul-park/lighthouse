//! Text edits: the form every fix is lowered to. Edits address byte ranges of
//! one file's original text; overlapping proposals are dropped before any is
//! applied, and the rest are applied in one pass.

use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use similar::TextDiff;

/// Replaces `range` of `file`'s text with `text`; an empty range inserts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TextEdit {
    pub file: PathBuf,
    pub range: Range<usize>,
    pub text: String,
    /// Where in the file the text came from, for inserts at one place: they
    /// are applied in the order of their sources, however the edits were
    /// listed. Zero when it says nothing.
    #[serde(default)]
    pub rank: usize,
}

impl TextEdit {
    pub(crate) fn delete(file: PathBuf, range: Range<usize>) -> Self {
        Self {
            file,
            range,
            text: String::new(),
            rank: 0,
        }
    }

    pub(crate) fn insert(file: PathBuf, at: usize, text: String) -> Self {
        Self {
            file,
            range: at..at,
            text,
            rank: 0,
        }
    }

    /// An insert of text that came from `rank` in the file, see [`TextEdit::rank`].
    pub(crate) fn insert_from(file: PathBuf, at: usize, text: String, rank: usize) -> Self {
        Self {
            rank,
            ..Self::insert(file, at, text)
        }
    }
}

/// The unified diff from `before` to `after` of one file; empty when equal.
pub fn unified_diff(path: &Path, before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }
    let name = path.to_string_lossy().replace('\\', "/");
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string()
}

/// The smallest edit of whole lines that turns `old` into `new`; `None` when
/// they are the same.
pub(crate) fn replace_changed(file: &Path, old: &str, new: &str) -> Option<TextEdit> {
    if old == new {
        return None;
    }
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let size = |lines: &[&str]| lines.iter().map(|l| l.len()).sum::<usize>();
    let start = size(&old_lines[..prefix]);
    let end = old.len() - size(&old_lines[old_lines.len() - suffix..]);
    let from = size(&new_lines[..prefix]);
    let to = new.len() - size(&new_lines[new_lines.len() - suffix..]);
    Some(TextEdit {
        file: file.to_owned(),
        range: start..end,
        text: new[from..to].to_owned(),
        rank: 0,
    })
}

/// Whether two edits of one file collide. Two inserts at one point do not; an
/// insert strictly inside a replaced range does.
pub(crate) fn overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => false,
        (true, false) => b.start < a.start && a.start < b.end,
        (false, true) => a.start < b.start && b.start < a.end,
        (false, false) => a.start < b.end && b.start < a.end,
    }
}

/// Applies edits of one file to its text. Edits at the same offset apply by their
/// `rank`, then in the order given. Ranges that are out of bounds, inside a character, or
/// that overlap are an error: nothing here panics.
pub(crate) fn apply(text: &str, edits: &[&TextEdit]) -> Result<String, String> {
    let mut order: Vec<(usize, &&TextEdit)> = edits.iter().enumerate().collect();
    order.sort_by_key(|(n, e)| (e.range.start, e.range.end, e.rank, *n));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (_, edit) in order {
        let Range { start, end } = edit.range;
        let sound = start <= end
            && end <= text.len()
            && start >= at
            && text.is_char_boundary(start)
            && text.is_char_boundary(end);
        if !sound {
            return Err(format!(
                "an edit of {} covers {start}..{end}, which is out of place",
                edit.file.display()
            ));
        }
        out.push_str(&text[at..start]);
        out.push_str(&edit.text);
        at = end;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

/// Whether any two of the edits of one file collide.
pub(crate) fn self_overlap(edits: &[TextEdit]) -> bool {
    edits.iter().enumerate().any(|(n, a)| {
        edits[n + 1..]
            .iter()
            .any(|b| a.file == b.file && overlap(&a.range, &b.range))
    })
}

/// Whether the edits of a fix that only moves declarations leave the file a
/// permutation of what it had: the lines that hold text, counted with their
/// repeats, are the same before and after. Blank lines are not counted, since
/// the lowering takes or leaves one with each move to keep one where one
/// belonged. Else what the file lost or gained.
pub(crate) fn keeps_declarations(text: &str, edits: &[&TextEdit]) -> Result<(), String> {
    let after = apply(text, edits)?;
    let mut counts: BTreeMap<&str, isize> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        *counts.entry(line).or_default() -= 1;
    }
    for line in after.lines().filter(|l| !l.trim().is_empty()) {
        *counts.entry(line).or_default() += 1;
    }
    let Some((line, n)) = counts.iter().find(|(_, n)| **n != 0) else {
        return Ok(());
    };
    let (what, n) = if *n > 0 {
        ("repeats", *n)
    } else {
        ("drops", -*n)
    };
    Err(format!(
        "the reorder would not keep the file's declarations: it {what} `{}` {n} time(s)",
        line.trim()
    ))
}

/// The byte range of whole lines that `range` fills, extended by one blank
/// line so that removing it never leaves two or none where one belonged;
/// `None` when other text shares its first or last line.
pub(crate) fn whole_lines(text: &str, range: &Range<usize>) -> Option<Range<usize>> {
    let start = text[..range.start].rfind('\n').map_or(0, |n| n + 1);
    let end = if range.end > range.start && text.as_bytes()[range.end - 1] == b'\n' {
        range.end
    } else {
        text[range.end..]
            .find('\n')
            .map_or(text.len(), |n| range.end + n + 1)
    };
    let before_blank = text[start..range.start].trim().is_empty();
    let after_blank = text[range.end..end].trim().is_empty();
    if !(before_blank && after_blank) {
        return None;
    }
    Some(with_blank(text, start..end, None))
}

/// `lines` with the blank lines that go with it, so that what remains of the
/// scope it leaves keeps the separation it had: the gap between its neighbours
/// is one of the two it bridged, the one nearest `separation` (the larger when
/// the scope has none), never their sum and never a blank line against an
/// opening or a closing bracket. A declaration that closes its scope (a
/// bracket, a dedent or the end of the file follows) takes the blank lines
/// before it; one that opens it takes those after it.
pub(crate) fn with_blank(
    text: &str,
    lines: Range<usize>,
    separation: Option<usize>,
) -> Range<usize> {
    let above = blank_start(text, lines.start)..lines.start;
    let below = lines.end..blank_end(text, lines.end);
    let indent = text[lines.clone()]
        .lines()
        .find(|l| !l.trim().is_empty())
        .map_or(0, indent_of);
    let first = text[..above.start]
        .trim_end()
        .lines()
        .next_back()
        .is_none_or(|line| indent_of(line) < indent || line.trim_end().ends_with(['{', '(', '[']));
    let last = text[below.end..]
        .lines()
        .find(|l| !l.trim().is_empty())
        .is_none_or(|line| {
            indent_of(line) < indent || line.trim_start().starts_with(['}', ')', ']'])
        });
    match (first, last) {
        (true, true) => above.start..below.end,
        (true, false) => lines.start..below.end,
        (false, true) => without_final_eol(text, above.start..lines.end),
        (false, false) => {
            let (before, after) = (blank_count(text, &above), blank_count(text, &below));
            let gap = separation.map_or(before.max(after), |s| {
                s.clamp(before.min(after), before.max(after))
            });
            let taken = before + after - gap;
            let from_below = taken.min(after);
            let from_above = lines_len_back(&text[above.clone()], taken - from_below);
            lines.start - from_above..below.start + lines_len(&text[below.clone()], from_below)
        }
    }
}

/// The start of the blank lines that end at `at`, a line start.
pub(crate) fn blank_start(text: &str, at: usize) -> usize {
    let mut start = at;
    while start > 0 {
        let line = text[..start - 1].rfind('\n').map_or(0, |n| n + 1);
        if !text[line..start - 1].trim().is_empty() {
            break;
        }
        start = line;
    }
    start
}

/// The end of the blank lines that begin at `at`, a line start.
pub(crate) fn blank_end(text: &str, at: usize) -> usize {
    let mut end = at;
    while end < text.len() {
        let next = text[end..].find('\n').map_or(text.len(), |n| end + n + 1);
        if !text[end..next].trim().is_empty() {
            break;
        }
        end = next;
    }
    end
}

/// How many blank lines `run` holds.
pub(crate) fn blank_count(text: &str, run: &Range<usize>) -> usize {
    text[run.clone()].matches('\n').count()
}

/// The width of the whitespace that leads `line`.
pub(crate) fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The byte length of the first `count` lines of `run`.
fn lines_len(run: &str, count: usize) -> usize {
    run.split_inclusive('\n').take(count).map(str::len).sum()
}

/// The byte length of the last `count` lines of `run`.
fn lines_len_back(run: &str, count: usize) -> usize {
    run.split_inclusive('\n')
        .rev()
        .take(count)
        .map(str::len)
        .sum()
}

/// `range`, which runs to the end of the text, widened by the line end before
/// it when the text has no line end of its own: what remains ends as the text
/// did.
fn without_final_eol(text: &str, range: Range<usize>) -> Range<usize> {
    if range.end < text.len() || text.ends_with('\n') || range.start == 0 {
        return range;
    }
    let eol = if text[..range.start].ends_with("\r\n") {
        2
    } else {
        1
    };
    range.start - eol..range.end
}

#[cfg(test)]
mod tests;
