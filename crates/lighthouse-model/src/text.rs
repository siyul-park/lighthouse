use crate::{Position, Span};

/// Converts between byte offsets of a text and the 1-based line and byte
/// column positions spans use.
#[derive(Debug, Clone)]
pub struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    /// Indexes the lines of `text`.
    pub fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        Self { text, starts }
    }

    /// The byte offset of a position; `None` when it lies outside the text.
    /// A position just past the last line's end, as an exclusive end is, maps
    /// to the length of the text.
    pub fn offset(&self, at: Position) -> Option<usize> {
        if at.line as usize == self.starts.len() + 1 && at.col == 1 {
            return Some(self.text.len());
        }
        let start = *self.starts.get(at.line.checked_sub(1)? as usize)?;
        let offset = start.checked_add(at.col.checked_sub(1)? as usize)?;
        let line_end = self
            .starts
            .get(at.line as usize)
            .copied()
            .unwrap_or(self.text.len());
        (offset <= line_end && self.text.is_char_boundary(offset)).then_some(offset)
    }

    /// The byte range of a span.
    pub fn range(&self, span: Span) -> Option<std::ops::Range<usize>> {
        let (start, end) = (self.offset(span.start)?, self.offset(span.end)?);
        (start <= end).then_some(start..end)
    }

    /// The position of a byte offset.
    pub fn position(&self, offset: usize) -> Position {
        let line = self.starts.partition_point(|&start| start <= offset);
        let start = self.starts[line.saturating_sub(1)];
        Position {
            line: u32::try_from(line.max(1)).unwrap_or(u32::MAX),
            col: u32::try_from(offset - start + 1).unwrap_or(u32::MAX),
        }
    }

    /// The offset where the line holding `offset` starts.
    pub fn line_start(&self, offset: usize) -> usize {
        let line = self.starts.partition_point(|&start| start <= offset);
        self.starts[line.saturating_sub(1)]
    }

    /// The offset just past the newline that ends the line holding `offset`,
    /// or the end of the text.
    pub fn line_end(&self, offset: usize) -> usize {
        let line = self.starts.partition_point(|&start| start <= offset);
        self.starts.get(line).copied().unwrap_or(self.text.len())
    }
}
