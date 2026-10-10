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

struct Cursor<'a> {
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
    line: u32,
    line_start: usize,
}

impl Iterator for Cursor<'_> {
    type Item = Found;

    fn next(&mut self) -> Option<Found> {
        while self.at < self.bytes.len() {
            match (self.bytes[self.at], self.peek(1)) {
                (b'/', Some(b'/')) => return Some(self.line_comment()),
                (b'/', Some(b'*')) => return Some(self.block_comment()),
                (b'"', _) => self.string(),
                (b'r' | b'b', _) if self.raw_string_start().is_some() => self.raw_string(),
                (b'\'', _) => self.quote(),
                _ => self.bump(),
            }
        }
        None
    }
}

impl Cursor<'_> {
    fn peek(&self, ahead: usize) -> Option<u8> {
        self.bytes.get(self.at + ahead).copied()
    }

    /// A `'` starts a character literal or a lifetime; only the literal can hide
    /// a `/`.
    fn quote(&mut self) {
        self.bump();
        match (self.peek(0), self.peek(1)) {
            (Some(b'\\'), _) => {
                while self.at < self.bytes.len() && self.bytes[self.at] != b'\'' {
                    self.bump();
                }
                if self.at < self.bytes.len() {
                    self.bump();
                }
            }
            (Some(c), Some(b'\'')) if c != b'\n' => {
                self.bump();
                self.bump();
            }
            _ => {}
        }
    }

    /// Moves one byte, tracking lines.
    fn bump(&mut self) {
        if self.bytes[self.at] == b'\n' {
            self.line += 1;
            self.line_start = self.at + 1;
        }
        self.at += 1;
    }

    /// Number of `#` of a raw string starting at the cursor (`r"`, `r#"`, `br"`),
    /// and the length of its prefix, when one starts here and is not the tail of
    /// an identifier.
    fn raw_string_start(&self) -> Option<(usize, usize)> {
        let before = self.at.checked_sub(1).and_then(|i| self.bytes.get(i));
        if before.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') {
            return None;
        }
        let mut at = 0;
        if self.peek(at) == Some(b'b') {
            at += 1;
        }
        if self.peek(at) != Some(b'r') {
            return None;
        }
        at += 1;
        let mut hashes = 0;
        while self.peek(at + hashes) == Some(b'#') {
            hashes += 1;
        }
        (self.peek(at + hashes) == Some(b'"')).then_some((hashes, at + hashes + 1))
    }

    fn line_comment(&mut self) -> Found {
        let start = self.location();
        let standalone = self.standalone();
        let from = self.at;
        while self.at < self.bytes.len() && self.bytes[self.at] != b'\n' {
            self.bump();
        }
        let mut body = &self.text[from..self.at];
        body = body.strip_suffix('\r').unwrap_or(body);
        Found {
            start,
            end: self.location(),
            text: body.to_owned(),
            standalone,
        }
    }

    fn location(&self) -> (u32, u32) {
        let col = self.at - self.line_start + 1;
        (self.line, u32::try_from(col).unwrap_or(u32::MAX))
    }

    fn standalone(&self) -> bool {
        self.text[self.line_start..self.at].trim().is_empty()
    }

    fn block_comment(&mut self) -> Found {
        let start = self.location();
        let standalone = self.standalone();
        let from = self.at;
        let mut depth = 0usize;
        while self.at < self.bytes.len() {
            match (self.bytes[self.at], self.peek(1)) {
                (b'/', Some(b'*')) => {
                    depth += 1;
                    self.bump();
                    self.bump();
                }
                (b'*', Some(b'/')) => {
                    depth -= 1;
                    self.bump();
                    self.bump();
                    if depth == 0 {
                        break;
                    }
                }
                _ => self.bump(),
            }
        }
        Found {
            start,
            end: self.location(),
            text: self.text[from..self.at].to_owned(),
            standalone,
        }
    }

    fn string(&mut self) {
        self.bump();
        while self.at < self.bytes.len() {
            match self.bytes[self.at] {
                b'\\' => {
                    self.bump();
                    if self.at < self.bytes.len() {
                        self.bump();
                    }
                }
                b'"' => {
                    self.bump();
                    return;
                }
                _ => self.bump(),
            }
        }
    }

    fn raw_string(&mut self) {
        let Some((hashes, prefix)) = self.raw_string_start() else {
            self.bump();
            return;
        };
        for _ in 0..prefix {
            self.bump();
        }
        while self.at < self.bytes.len() {
            if self.bytes[self.at] == b'"' && (1..=hashes).all(|i| self.peek(i) == Some(b'#')) {
                for _ in 0..=hashes {
                    self.bump();
                }
                return;
            }
            self.bump();
        }
    }
}

/// The comments of `text` in source order; adjacent standalone line comments
/// form one comment. A comment directly above a symbol, with at most
/// attributes between them, is attached to the first symbol that starts
/// there.
pub fn scan(text: &str, symbols: &[Symbol]) -> Vec<Comment> {
    let lines: Vec<&str> = text.split('\n').collect();
    let cursor = Cursor {
        text,
        bytes: text.as_bytes(),
        at: 0,
        line: 1,
        line_start: 0,
    };
    merge(cursor.collect())
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

/// Draws each symbol's extent up over the comments that lead it: a standalone
/// comment that ends on the line before the extent starts, again and again,
/// so a plain comment above doc comments and attributes is the item's too.
pub fn widen_extents(text: &str, comments: &[Comment], symbols: &mut [Symbol]) {
    let lines: Vec<&str> = text.split('\n').collect();
    let standalone = |c: &Comment| {
        lines
            .get(c.span.start.line as usize - 1)
            .and_then(|l| l.get(..c.span.start.col as usize - 1))
            .is_some_and(|before| before.trim().is_empty())
    };
    for symbol in symbols {
        let Some(extent) = &mut symbol.extent else {
            continue;
        };
        while let Some(comment) = comments
            .iter()
            .find(|c| c.span.end.line + 1 == extent.start.line && standalone(c) && leads(&c.text))
        {
            extent.start = comment.span.start;
        }
    }
}

/// Whether a comment can belong to the item below it: not an inner doc comment
/// (`//!`, `/*!`), which documents the module around, and not a banner, whose
/// line is a run of rule characters or a section marker.
fn leads(text: &str) -> bool {
    if text.starts_with("//!") || text.starts_with("/*!") {
        return false;
    }
    let rule = |c: char| "-=*~_+".contains(c);
    let banner = text.lines().all(|line| {
        let body = line.trim().trim_start_matches(['/', '*', '#', '!']).trim();
        let run = body.chars().take_while(|&c| rule(c)).count();
        body.is_empty() || run >= 3 || body.starts_with("MARK:") || body.starts_with("#region")
    });
    !banner
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

    fn symbol_at(from: (u32, u32), to: (u32, u32)) -> Symbol {
        let at = |(line, col)| Position { line, col };
        let span = Span {
            start: at(from),
            end: at(to),
        };
        Symbol {
            id: "m::f#function".to_owned(),
            kind: lighthouse_protocol::SymbolKind::Function,
            visibility: lighthouse_protocol::Visibility::Private,
            owner: None,
            file: "a.rs".to_owned(),
            span,
            extent: Some(span),
            doc: None,
            name: "f".to_owned(),
            role: None,
            optional: false,
            type_ref: None,
        }
    }

    #[test]
    fn an_extent_grows_over_every_standalone_comment_that_leads_it() {
        let source = "// far\n\n// a\n// b\n/// doc\nfn f() {}\n";
        let mut symbols = [symbol_at((5, 1), (6, 8))];

        widen_extents(source, &scan(source, &symbols), &mut symbols);

        let start = symbols[0].extent.unwrap().start;
        assert_eq!((start.line, start.col), (3, 1), "the blank line stops it");
    }

    #[test]
    fn inner_docs_and_banners_stay_out_of_an_extent() {
        let inner = "//! module docs\nfn f() {}\n";
        let banner = "// ---- Helpers ----\nfn f() {}\n";
        let marker = "// MARK: - Helpers\nfn f() {}\n";
        let prose = "// ---- not a banner: it explains\n// the item\nfn f() {}\n";

        for (source, line, want) in [(inner, 2, 2), (banner, 2, 2), (marker, 2, 2), (prose, 3, 1)] {
            let mut symbols = [symbol_at((line, 1), (line, 8))];
            widen_extents(source, &scan(source, &symbols), &mut symbols);
            assert_eq!(symbols[0].extent.unwrap().start.line, want, "{source}");
        }
    }

    #[test]
    fn a_comment_that_trails_code_does_not_belong_to_the_next_item() {
        let source = "let x = 1; // trailing\nfn f() {}\n";
        let mut symbols = [symbol_at((2, 1), (2, 10))];

        widen_extents(source, &scan(source, &symbols), &mut symbols);

        assert_eq!(symbols[0].extent.unwrap().start.line, 2);
    }
}
