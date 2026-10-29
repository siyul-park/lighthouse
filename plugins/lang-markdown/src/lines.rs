//! The lines of a text, as CommonMark ends them (`\n`, `\r\n` or a lone `\r`),
//! and the byte offsets the parser's line and column positions stand for.

pub(crate) struct Lines<'t> {
    text: &'t str,
    /// The offset at which each line starts.
    starts: Vec<usize>,
}

impl<'t> Lines<'t> {
    pub(crate) fn new(text: &'t str) -> Self {
        let bytes = text.as_bytes();
        let mut starts = vec![0];
        for (at, byte) in bytes.iter().enumerate() {
            let ends = match byte {
                b'\n' => true,
                b'\r' => bytes.get(at + 1) != Some(&b'\n'),
                _ => false,
            };
            if ends {
                starts.push(at + 1);
            }
        }
        Self { text, starts }
    }

    pub(crate) fn text(&self) -> &'t str {
        self.text
    }

    /// The offset at which the one-based `line` starts; the end of the text
    /// for a line past the last.
    pub(crate) fn start(&self, line: usize) -> usize {
        line.checked_sub(1)
            .and_then(|at| self.starts.get(at))
            .copied()
            .unwrap_or(self.text.len())
    }

    /// The offset after the line break of the one-based `line`.
    pub(crate) fn end(&self, line: usize) -> usize {
        self.start(line + 1)
    }

    /// The half-open byte range from the position `from` to the position
    /// `to`, both `(line, column)` with the column counted in bytes from one
    /// and the end column that of the last byte (zero: the end of the line
    /// before). `None` when the positions are not inside the text.
    pub(crate) fn range(&self, from: (usize, usize), to: (usize, usize)) -> Option<[usize; 2]> {
        if from.0 == 0 || from.1 == 0 || to.0 == 0 {
            return None;
        }
        let start = self.start(from.0) + (from.1 - 1);
        let end = self.start(to.0) + to.1;
        let inside = start <= end
            && end <= self.text.len()
            && self.text.is_char_boundary(start)
            && self.text.is_char_boundary(end);
        inside.then_some([start, end])
    }
}
