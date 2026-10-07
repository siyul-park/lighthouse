//! Which bracket a declaration sits in. Moving or reordering is only safe
//! between declarations of the same container: the same file level, the same
//! `impl` or class body. The scan reads brackets the way C-family and Rust
//! sources write them, skipping comments, strings and characters, so it needs
//! no parser; a language that does not bracket its bodies has one container,
//! and the re-check after the edit is what catches a wrong guess.

/// The innermost bracket pair around an offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Container {
    /// Offset of the opening bracket; `None` at file level.
    pub open: Option<usize>,
    /// A `(` group, such as Go's `const ( ... )` block: its members depend on
    /// their place and are never moved.
    pub group: bool,
}

struct Frame {
    open: usize,
    close: usize,
    group: bool,
}

/// The bracket structure of one text.
pub(crate) struct Containers {
    frames: Vec<Frame>,
}

impl Containers {
    /// `language` decides what a comment or a string looks like: block
    /// comments nest and raw strings use `r#"..."#` only in Rust, a backtick
    /// quotes only in Go and the other languages that have raw strings; an
    /// unknown language is read like C.
    pub(crate) fn new(text: &str, language: &str) -> Self {
        let rust = language == "rust";
        let bytes = text.as_bytes();
        let mut frames = Vec::new();
        let mut stack: Vec<(usize, u8)> = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let byte = bytes[at];
            at = match byte {
                b'/' if bytes.get(at + 1) == Some(&b'/') => line_end(bytes, at),
                b'/' if bytes.get(at + 1) == Some(&b'*') => block_end(bytes, at, rust),
                b'"' => quoted_end(bytes, at, b'"'),
                b'`' if !rust => quoted_end(bytes, at, b'`'),
                b'r' if rust && raw_string(bytes, at).is_some() => {
                    raw_string(bytes, at).unwrap_or(at + 1)
                }
                b'\'' if rust || language == "go" => char_end(bytes, at),
                b'{' | b'(' | b'[' => {
                    stack.push((at, byte));
                    at + 1
                }
                b'}' | b')' | b']' => {
                    if let Some((open, kind)) = stack.pop()
                        && closes(kind, byte)
                    {
                        frames.push(Frame {
                            open,
                            close: at,
                            group: kind == b'(',
                        });
                    }
                    at + 1
                }
                _ => at + 1,
            };
        }
        Self { frames }
    }

    /// The innermost pair that has `offset` strictly between its brackets.
    pub(crate) fn at(&self, offset: usize) -> Container {
        let inner = self
            .frames
            .iter()
            .filter(|f| f.open < offset && offset <= f.close)
            .max_by_key(|f| f.open);
        Container {
            open: inner.map(|f| f.open),
            group: inner.is_some_and(|f| f.group),
        }
    }
}

fn closes(open: u8, close: u8) -> bool {
    matches!((open, close), (b'{', b'}') | (b'(', b')') | (b'[', b']'))
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(bytes.len(), |n| from + n)
}

/// The end of a block comment; Rust nests them, the others end at the first `*/`.
fn block_end(bytes: &[u8], from: usize, nested: bool) -> usize {
    let (mut depth, mut at) = (0usize, from);
    while at < bytes.len() {
        if bytes[at..].starts_with(b"/*") && (nested || depth == 0) {
            depth += 1;
            at += 2;
        } else if bytes[at..].starts_with(b"*/") {
            depth -= 1;
            at += 2;
            if depth == 0 {
                return at;
            }
        } else {
            at += 1;
        }
    }
    bytes.len()
}

fn quoted_end(bytes: &[u8], from: usize, quote: u8) -> usize {
    let mut at = from + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' if quote == b'"' => at += 2,
            b if b == quote => return at + 1,
            _ => at += 1,
        }
    }
    bytes.len()
}

/// The end of a Rust raw string `r#"..."#` starting at `from`.
fn raw_string(bytes: &[u8], from: usize) -> Option<usize> {
    let hashes = bytes[from + 1..].iter().take_while(|&&b| b == b'#').count();
    if bytes.get(from + 1 + hashes) != Some(&b'"') {
        return None;
    }
    if from > 0 && (bytes[from - 1].is_ascii_alphanumeric() || bytes[from - 1] == b'_') {
        return None;
    }
    let mut close = vec![b'"'];
    close.extend(std::iter::repeat_n(b'#', hashes));
    let body = from + 2 + hashes;
    bytes[body..]
        .windows(close.len())
        .position(|w| w == close.as_slice())
        .map(|n| body + n + close.len())
}

/// The end of a character literal, or just past the quote when it is a
/// lifetime or a label.
fn char_end(bytes: &[u8], from: usize) -> usize {
    let rest = &bytes[from + 1..];
    let end = match rest.first() {
        Some(b'\\') => rest.iter().skip(1).position(|&b| b == b'\'').map(|n| n + 2),
        Some(_) => {
            let width = utf8_width(rest[0]);
            (rest.get(width) == Some(&b'\'')).then_some(width + 1)
        }
        None => None,
    };
    end.map_or(from + 1, |n| from + 1 + n)
}

fn utf8_width(first: u8) -> usize {
    match first {
        0xF0.. => 4,
        0xE0.. => 3,
        0xC0.. => 2,
        _ => 1,
    }
}
