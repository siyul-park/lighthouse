//! Text edits: the form every fix is lowered to. Edits address byte ranges of
//! one file's original text; overlapping proposals are dropped before any is
//! applied, and the rest are applied in one pass.

use std::{ops::Range, path::PathBuf};

/// Replaces `range` of `file`'s text with `text`; an empty range inserts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextEdit {
    pub file: PathBuf,
    pub range: Range<usize>,
    pub text: String,
}

impl TextEdit {
    pub(crate) fn delete(file: PathBuf, range: Range<usize>) -> Self {
        Self {
            file,
            range,
            text: String::new(),
        }
    }

    pub(crate) fn insert(file: PathBuf, at: usize, text: String) -> Self {
        Self {
            file,
            range: at..at,
            text,
        }
    }
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

/// The dominant line ending of a text: CRLF when most of its lines end in it.
pub(crate) fn eol_of(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count();
    if lf > 0 && crlf * 2 > lf {
        "\r\n"
    } else {
        "\n"
    }
}

/// Applies edits of one file to its text. Edits at the same offset apply in
/// the order given. Ranges that are out of bounds, inside a character, or
/// that overlap are an error: nothing here panics.
pub(crate) fn apply(text: &str, edits: &[&TextEdit]) -> Result<String, String> {
    let mut order: Vec<(usize, &&TextEdit)> = edits.iter().enumerate().collect();
    order.sort_by_key(|(n, e)| (e.range.start, e.range.end, *n));
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
    Some(with_blank(text, start..end))
}

/// `lines`, widened by the blank line that follows it, else by the one that
/// precedes it when nothing follows or a closing bracket does: what remains
/// has one blank line where one belonged, never two and never a blank line
/// against a bracket.
pub(crate) fn with_blank(text: &str, lines: Range<usize>) -> Range<usize> {
    let line_after = |from: usize| {
        let end = text[from..].find('\n').map_or(text.len(), |n| from + n + 1);
        &text[from..end]
    };
    if lines.end < text.len() && line_after(lines.end).trim().is_empty() {
        return lines.start..lines.end + line_after(lines.end).len();
    }
    let closes = lines.end >= text.len()
        || line_after(lines.end)
            .trim_start()
            .starts_with(['}', ')', ']']);
    let before = &text[..lines.start];
    if closes && let Some(prev) = before.strip_suffix('\n') {
        let prev_start = prev.rfind('\n').map_or(0, |n| n + 1);
        if before[prev_start..].trim().is_empty() {
            return prev_start..lines.end;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_treats_inserts_at_one_point_as_compatible() {
        assert!(!overlap(&(3..3), &(3..3)));
        assert!(!overlap(&(3..3), &(3..5)));
        assert!(overlap(&(4..4), &(3..5)));
        assert!(overlap(&(1..4), &(3..5)));
        assert!(!overlap(&(1..3), &(3..5)));
    }

    #[test]
    fn apply_rebuilds_the_text_in_one_pass() {
        let delete = TextEdit::delete(PathBuf::new(), 2..4);
        let insert = TextEdit::insert(PathBuf::new(), 6, "X".to_owned());
        assert_eq!(apply("abcdefgh", &[&insert, &delete]).unwrap(), "abefXgh");
    }

    #[test]
    fn whole_lines_take_a_following_blank_line() {
        let text = "a\n\nb\n\nc\n";
        assert_eq!(whole_lines(text, &(3..4)), Some(3..6));
        assert_eq!(whole_lines(text, &(0..1)), Some(0..3));
    }

    #[test]
    fn whole_lines_fall_back_to_the_preceding_blank_line() {
        let text = "a\n\nb\n";
        assert_eq!(whole_lines(text, &(3..4)), Some(2..5));
    }

    #[test]
    fn a_preceding_blank_line_stays_when_code_follows() {
        let text = "a\n\nb\nc\n";
        assert_eq!(whole_lines(text, &(3..4)), Some(3..5));
    }

    #[test]
    fn a_preceding_blank_line_goes_when_a_bracket_follows() {
        let text = "{\n\nb\n}\n";
        assert_eq!(whole_lines(text, &(3..4)), Some(2..5));
    }

    #[test]
    fn a_range_that_shares_its_line_is_not_whole_lines() {
        assert_eq!(whole_lines("x := 1 // c\n", &(7..11)), None);
    }
}
