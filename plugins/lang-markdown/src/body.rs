//! Where the text of a code block lies in the document: the parser strips the
//! indentation a list or a blockquote adds, and a fence's own offset, from
//! each line of the block, so the text is mapped back line by line.

use lighthouse_protocol::TextMapSegment;

use crate::lines::Lines;

/// What the parser says of a code block's place.
pub(crate) struct Block<'a> {
    pub(crate) fenced: bool,
    /// The fence has a closing marker.
    pub(crate) closed: bool,
    /// The lines the block spans, one-based and inclusive.
    pub(crate) lines: (usize, usize),
    /// The text the parser read from its body.
    pub(crate) literal: &'a str,
}

/// The lines of a block's body in the document, and the text read from them.
pub(crate) struct Body {
    /// Where the body lies; the whole lines it was read from when `map` is
    /// `None`.
    pub(crate) range: [usize; 2],
    /// Which bytes of the text came from which bytes of the document, `None`
    /// when a line cannot be matched to its source exactly.
    pub(crate) map: Option<Vec<TextMapSegment>>,
}

/// The body of `block`.
pub(crate) fn locate(lines: &Lines, block: &Block) -> Body {
    let (start, end) = block.lines;
    let first = if block.fenced { start + 1 } else { start };
    let last = if block.fenced && block.closed {
        end.saturating_sub(1)
    } else {
        end
    };
    if first > last || block.literal.is_empty() {
        let at = lines.start(first);
        return Body {
            range: [at, at],
            map: Some(Vec::new()),
        };
    }
    let whole = [lines.start(first), lines.end(last)];
    match segments(lines, block.literal, first, last) {
        Some(map) if !map.is_empty() => Body {
            range: [
                map[0].source_range[0] as usize,
                map[map.len() - 1].source_range[1] as usize,
            ],
            map: Some(map),
        },
        _ => Body {
            range: whole,
            map: None,
        },
    }
}

/// The segments that map each line of `literal` to the end of its line of the
/// document, `None` unless every line matches.
fn segments(
    lines: &Lines,
    literal: &str,
    first: usize,
    last: usize,
) -> Option<Vec<TextMapSegment>> {
    let text = lines.text();
    let mut segments: Vec<TextMapSegment> = Vec::new();
    let mut offset = 0;
    for (line, read) in (first..=last).zip(pieces(literal)) {
        let (start, end) = (lines.start(line), lines.end(line));
        let source = &text[start..end];
        // The last line of a document has no break, and the parser adds one.
        let kept = if source.ends_with(read) {
            read
        } else if end == text.len() {
            read.trim_end_matches(['\n', '\r'])
        } else {
            return None;
        };
        if !source.ends_with(kept) {
            return None;
        }
        push(
            &mut segments,
            [offset, offset + kept.len()],
            [end - kept.len(), end],
        );
        if kept.len() < read.len() {
            let at = offset + kept.len();
            push(&mut segments, [at, offset + read.len()], [end, end]);
        }
        offset += read.len();
    }
    (offset == literal.len()).then_some(segments)
}

/// The pieces of `text`, each with the line break that ends it.
fn pieces(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut pieces = Vec::new();
    let mut from = 0;
    let mut at = 0;
    while at < bytes.len() {
        let width = match bytes[at] {
            b'\n' => 1,
            b'\r' if bytes.get(at + 1) == Some(&b'\n') => 2,
            b'\r' => 1,
            _ => {
                at += 1;
                continue;
            }
        };
        at += width;
        pieces.push(&text[from..at]);
        from = at;
    }
    if from < text.len() {
        pieces.push(&text[from..]);
    }
    pieces
}

/// Adds a segment, joined to the one before it when both lie side by side and
/// are as long in the text as in the document.
fn push(segments: &mut Vec<TextMapSegment>, text: [usize; 2], source: [usize; 2]) {
    let segment = TextMapSegment {
        text_range: text.map(|at| at as u32),
        source_range: source.map(|at| at as u32),
    };
    if text[0] == text[1] && source[0] == source[1] {
        return;
    }
    let linear = |s: &TextMapSegment| {
        s.text_range[1] - s.text_range[0] == s.source_range[1] - s.source_range[0]
    };
    if let Some(previous) = segments.last_mut()
        && previous.text_range[1] == segment.text_range[0]
        && previous.source_range[1] == segment.source_range[0]
        && linear(previous)
        && linear(&segment)
    {
        previous.text_range[1] = segment.text_range[1];
        previous.source_range[1] = segment.source_range[1];
        return;
    }
    segments.push(segment);
}
