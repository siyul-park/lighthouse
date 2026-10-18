//! The extent of a finding: the code an agent may change when the scope of
//! its fix is `symbol`. It is the symbol the finding is about, else the
//! innermost one around it, else a small file as a whole.

use lighthouse_model::{Diagnostic, LineIndex, Position, Span, ucm::Project};

/// A file with this many lines or more has no extent without a symbol.
const WHOLE_FILE_LINES: usize = 400;

/// The span of the extent of `finding` in `text`, the text of its file; or
/// why there is none.
pub(in crate::fix) fn choose(
    project: &Project,
    finding: &Diagnostic,
    text: &str,
) -> Result<Span, String> {
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
    index
        .range(span)
        .map(|_| span)
        .ok_or_else(|| "the extent lies outside the file".to_owned())
}
