//! The code a model is asked to rewrite for one finding: the symbol the
//! finding is about, else the innermost one around it, else a small file as
//! a whole.

use lighthouse_model::{Diagnostic, LineIndex, Position, Span, ucm::Project};

/// A file with this many lines or more is not rewritten as a whole.
const WHOLE_FILE_LINES: usize = 400;

/// What the model rewrites, and where it is.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Extent {
    pub(super) span: Span,
    pub(super) text: String,
}

impl Extent {
    /// The line and column of `at` counted from the start of the extent, or
    /// `None` when it lies before it.
    pub(super) fn relative(&self, at: Position) -> Option<Position> {
        let start = self.span.start;
        match at.line.cmp(&start.line) {
            std::cmp::Ordering::Less => None,
            std::cmp::Ordering::Equal => at.col.checked_sub(start.col).map(|col| Position {
                line: 1,
                col: col + 1,
            }),
            std::cmp::Ordering::Greater => Some(Position {
                line: at.line - start.line + 1,
                col: at.col,
            }),
        }
    }
}

/// The extent of `finding` in `text`, the text of its file; or why there is
/// none.
pub(super) fn choose(
    project: &Project,
    finding: &Diagnostic,
    text: &str,
) -> Result<Extent, String> {
    let index = LineIndex::new(text);
    let span = match project.anchor(finding) {
        Some(symbol) => symbol.extent.unwrap_or(symbol.span),
        None if text.lines().count() < WHOLE_FILE_LINES => Span {
            start: Position { line: 1, col: 1 },
            end: index.position(text.len()),
        },
        None => {
            return Err("too large for a model fix without a symbol".to_owned());
        }
    };
    let range = index
        .range(span)
        .ok_or_else(|| "the extent lies outside the file".to_owned())?;
    Ok(Extent {
        span,
        text: text[range].to_owned(),
    })
}
