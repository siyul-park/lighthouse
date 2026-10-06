//! Comments of a source file. syn drops everything but doc comments, so the
//! text is scanned directly: strings, raw strings and character literals are
//! skipped so that `"//"` in a literal is no comment.

use lighthouse_protocol::{Comment, Position, Span, Symbol};

/// Where a line comment run or block comment sits.
struct Found {
    start: (u32, u32),
    end: (u32, u32),
    text: String,
    /// Only whitespace precedes the comment on its first line.
    standalone: bool,
}

/// The comments of `text` in source order; adjacent standalone line comments
/// form one comment. A comment directly above a symbol, with at most
/// attributes between them, is attached to the first symbol that starts
/// there.
pub fn scan(text: &str, symbols: &[Symbol]) -> Vec<Comment> {
    let lines: Vec<&str> = text.split('\n').collect();
    merge(raw(text))
        .into_iter()
        .map(|c| Comment {
            attached_to: attached(&lines, c.end.0, symbols),
            span: Span {
                start: Position {
                    line: c.start.0,
                    col: c.start.1,
                },
                end: Position {
                    line: c.end.0,
                    col: c.end.1,
                },
            },
            text: c.text,
        })
        .collect()
}

fn attached(lines: &[&str], end_line: u32, symbols: &[Symbol]) -> Option<String> {
    let end = end_line as usize;
    let next = symbols
        .iter()
        .map(|s| s.span.start.line)
        .filter(|&l| l as usize > end)
        .min()?;
    let between = lines.get(end..next as usize - 1)?;
    let adjacent = between.iter().all(|l| l.trim_start().starts_with("#["));
    adjacent
        .then(|| symbols.iter().find(|s| s.span.start.line == next))
        .flatten()
        .map(|s| s.id.clone())
}

fn merge(found: Vec<Found>) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for next in found {
        let joins = out.last().is_some_and(|last| {
            last.standalone
                && next.standalone
                && last.text.starts_with("//")
                && next.text.starts_with("//")
                && next.start.0 == last.end.0 + 1
                && same_kind(&last.text, &next.text)
        });
        if joins {
            let last = out.last_mut().expect("joins implies a previous comment");
            last.text.push('\n');
            last.text.push_str(&next.text);
            last.end = next.end;
        } else {
            out.push(next);
        }
    }
    out
}

/// A `///` run does not join a plain `//` run: the first documents, the
/// second does not.
fn same_kind(a: &str, b: &str) -> bool {
    let kind = |t: &str| {
        let first = t.lines().next().unwrap_or("");
        (
            first.starts_with("///") && !first.starts_with("////"),
            first.starts_with("//!"),
        )
    };
    kind(a) == kind(b)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    line: u32,
    line_start: usize,
}

impl Cursor<'_> {
    fn position(&self) -> (u32, u32) {
        let col = self.at - self.line_start + 1;
        (self.line, u32::try_from(col).unwrap_or(u32::MAX))
    }

    fn peek(&self, ahead: usize) -> Option<u8> {
        self.bytes.get(self.at + ahead).copied()
    }

    /// Moves one byte, tracking lines.
    fn bump(&mut self) {
        if self.bytes[self.at] == b'\n' {
            self.line += 1;
            self.line_start = self.at + 1;
        }
        self.at += 1;
    }
}

fn raw(text: &str) -> Vec<Found> {
    let mut cur = Cursor {
        bytes: text.as_bytes(),
        at: 0,
        line: 1,
        line_start: 0,
    };
    let mut found = Vec::new();
    while cur.at < cur.bytes.len() {
        match (cur.bytes[cur.at], cur.peek(1)) {
            (b'/', Some(b'/')) => found.push(line_comment(&mut cur, text)),
            (b'/', Some(b'*')) => found.push(block_comment(&mut cur, text)),
            (b'"', _) => string(&mut cur),
            (b'r' | b'b', _) if raw_string_start(&cur).is_some() => raw_string(&mut cur),
            (b'\'', _) => quote(&mut cur),
            _ => cur.bump(),
        }
    }
    found
}

fn standalone(cur: &Cursor, text: &str) -> bool {
    text[cur.line_start..cur.at].trim().is_empty()
}

fn line_comment(cur: &mut Cursor, text: &str) -> Found {
    let start = cur.position();
    let standalone = standalone(cur, text);
    let from = cur.at;
    while cur.at < cur.bytes.len() && cur.bytes[cur.at] != b'\n' {
        cur.bump();
    }
    let mut body = &text[from..cur.at];
    body = body.strip_suffix('\r').unwrap_or(body);
    Found {
        start,
        end: cur.position(),
        text: body.to_owned(),
        standalone,
    }
}

fn block_comment(cur: &mut Cursor, text: &str) -> Found {
    let start = cur.position();
    let standalone = standalone(cur, text);
    let from = cur.at;
    let mut depth = 0usize;
    while cur.at < cur.bytes.len() {
        match (cur.bytes[cur.at], cur.peek(1)) {
            (b'/', Some(b'*')) => {
                depth += 1;
                cur.bump();
                cur.bump();
            }
            (b'*', Some(b'/')) => {
                depth -= 1;
                cur.bump();
                cur.bump();
                if depth == 0 {
                    break;
                }
            }
            _ => cur.bump(),
        }
    }
    Found {
        start,
        end: cur.position(),
        text: text[from..cur.at].to_owned(),
        standalone,
    }
}

fn string(cur: &mut Cursor) {
    cur.bump();
    while cur.at < cur.bytes.len() {
        match cur.bytes[cur.at] {
            b'\\' => {
                cur.bump();
                if cur.at < cur.bytes.len() {
                    cur.bump();
                }
            }
            b'"' => {
                cur.bump();
                return;
            }
            _ => cur.bump(),
        }
    }
}

/// Number of `#` of a raw string starting at the cursor (`r"`, `r#"`, `br"`),
/// and the length of its prefix, when one starts here and is not the tail of
/// an identifier.
fn raw_string_start(cur: &Cursor) -> Option<(usize, usize)> {
    let before = cur.at.checked_sub(1).and_then(|i| cur.bytes.get(i));
    if before.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') {
        return None;
    }
    let mut at = 0;
    if cur.peek(at) == Some(b'b') {
        at += 1;
    }
    if cur.peek(at) != Some(b'r') {
        return None;
    }
    at += 1;
    let mut hashes = 0;
    while cur.peek(at + hashes) == Some(b'#') {
        hashes += 1;
    }
    (cur.peek(at + hashes) == Some(b'"')).then_some((hashes, at + hashes + 1))
}

fn raw_string(cur: &mut Cursor) {
    let Some((hashes, prefix)) = raw_string_start(cur) else {
        cur.bump();
        return;
    };
    for _ in 0..prefix {
        cur.bump();
    }
    while cur.at < cur.bytes.len() {
        if cur.bytes[cur.at] == b'"' && (1..=hashes).all(|i| cur.peek(i) == Some(b'#')) {
            for _ in 0..=hashes {
                cur.bump();
            }
            return;
        }
        cur.bump();
    }
}

/// A `'` starts a character literal or a lifetime; only the literal can hide
/// a `/`.
fn quote(cur: &mut Cursor) {
    cur.bump();
    match (cur.peek(0), cur.peek(1)) {
        (Some(b'\\'), _) => {
            while cur.at < cur.bytes.len() && cur.bytes[cur.at] != b'\'' {
                cur.bump();
            }
            if cur.at < cur.bytes.len() {
                cur.bump();
            }
        }
        (Some(c), Some(b'\'')) if c != b'\n' => {
            cur.bump();
            cur.bump();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(source: &str) -> Vec<String> {
        scan(source, &[]).into_iter().map(|c| c.text).collect()
    }

    #[test]
    fn adjacent_line_comments_form_one_comment() {
        assert_eq!(
            texts("// a\n// b\nfn f() {}\n\n// c\n"),
            ["// a\n// b", "// c"]
        );
    }

    #[test]
    fn doc_and_plain_runs_stay_apart() {
        assert_eq!(texts("/// a\n// b\n"), ["/// a", "// b"]);
    }

    #[test]
    fn trailing_comments_do_not_join_the_next_line() {
        assert_eq!(texts("let x = 1; // a\n// b\n"), ["// a", "// b"]);
    }

    #[test]
    fn literals_hide_comment_markers() {
        let source =
            "let a = \"// no\"; let b = r#\"/* no\"#; let c = '/'; let d: &'a str; // yes\n";
        assert_eq!(texts(source), ["// yes"]);
    }

    #[test]
    fn block_comments_nest_and_keep_their_span() {
        let found = scan("x\n/* a /* b */ c */\n", &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "/* a /* b */ c */");
        assert_eq!((found[0].span.start.line, found[0].span.start.col), (2, 1));
        assert_eq!((found[0].span.end.line, found[0].span.end.col), (2, 18));
    }
}
