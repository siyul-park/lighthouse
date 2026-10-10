//! Rewrites the first form of the suppression directive, `lighthouse:allow`,
//! as its ESLint-style successor `lighthouse-disable-next-line` in source
//! files, and the ids of renamed decisions in every directive.
//!
//! Only comments are touched. A source file is read for its comments the way
//! its language writes them (strings, raw strings and character literals hide
//! a `//` from it), and a directive is rewritten wherever it starts a comment
//! line: a comment of its own, or one that trails code. Prose that mentions
//! the marker, strings, Markdown and the structured documents of Lighthouse
//! (YAML, TOML, JSON) are left as they are.

use std::{
    fs,
    ops::Range,
    path::{Path, PathBuf},
};

use lighthouse_model::annotation::MARKERS;

use super::{Action, Plan, file_name, walk};
use crate::Result;

const OLD: &str = "lighthouse:allow";
const NEW: &str = "lighthouse-disable-next-line";

/// How a language writes its comments and literals.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Go,
    Rust,
    /// `//` and `/* */`; strings in `"`, `'` and backticks.
    CLike,
    /// `#`; strings in `"` and `'`, triple quotes included.
    Hash,
}

/// Plans the rewrite of every source file under `paths`.
pub(super) fn rewrite(paths: &[PathBuf], plan: &mut Plan) -> Result<()> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            walk(path, &mut files)?;
        } else if path.is_file() {
            files.push(path.clone());
        }
    }
    files.sort();
    files.dedup();
    for path in files {
        let Some(family) = family(&path) else {
            continue;
        };
        if plan.actions.contains_key(&path) {
            continue;
        }
        // A file that is not UTF-8 text is not source we can edit.
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(text) = migrated(&text, family) {
            plan.actions.insert(path, Action::Write(text));
        }
    }
    Ok(())
}

fn family(path: &Path) -> Option<Family> {
    let name = file_name(path);
    if name.starts_with('.') {
        return None;
    }
    match name.rsplit_once('.')?.1 {
        "go" => Some(Family::Go),
        "rs" => Some(Family::Rust),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "java" | "kt" | "kts" | "swift" | "c"
        | "h" | "cc" | "cpp" | "hpp" | "cs" | "scala" | "dart" => Some(Family::CLike),
        "py" | "rb" | "sh" | "bash" | "zsh" => Some(Family::Hash),
        _ => None,
    }
}

/// `text` with every directive in its comments migrated; `None` when none is.
fn migrated(text: &str, family: Family) -> Option<String> {
    if !text.contains(OLD) && !text.contains("lighthouse-") {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut done = 0;
    for range in comments(text, family) {
        let comment = &text[range.clone()];
        let rewritten = comment_migrated(comment);
        if rewritten != comment {
            out.push_str(&text[done..range.start]);
            out.push_str(&rewritten);
            done = range.end;
        }
    }
    if done == 0 {
        return None;
    }
    out.push_str(&text[done..]);
    Some(out)
}

/// A comment with its directive lines migrated: the first line starts where
/// the comment does, the lines of a block comment after it are inside it.
fn comment_migrated(comment: &str) -> String {
    comment
        .split_inclusive('\n')
        .enumerate()
        .map(|(at, line)| migrated_line(line, at > 0).unwrap_or_else(|| line.to_owned()))
        .collect()
}

/// The line with the marker and the ids replaced, when the line is a comment
/// (or, with `inside`, in one) that starts with a directive.
fn migrated_line(line: &str, inside: bool) -> Option<String> {
    let body = line.trim_start_matches(|c: char| c.is_whitespace() || "/*#!".contains(c));
    let prefix = &line[..line.len() - body.len()];
    if !inside && !prefix.contains(['/', '*', '#']) {
        return None;
    }
    let (marker, after) = MARKERS.iter().find_map(|(marker, _)| {
        let after = body.strip_prefix(marker)?;
        (after.is_empty() || after.starts_with(char::is_whitespace)).then_some((*marker, after))
    })?;
    let marker = if marker == OLD { NEW } else { marker };
    let migrated = format!("{prefix}{marker}{}", renamed_ids(after));
    (migrated != line).then_some(migrated)
}

/// The ids of the directive, up to its ` -- ` reason, by the names they have
/// now; what follows them (the reason, a closing `*/`, the line end) stays.
fn renamed_ids(after: &str) -> String {
    let aliases = lighthouse_spec::Catalog::bundled().aliases();
    let trimmed = after.trim_end();
    let tail_at = trimmed.strip_suffix("*/").map_or(trimmed.len(), str::len);
    let (head, tail) = after.split_at(tail_at);
    let (ids, reason) = head
        .split_once(" --")
        .map_or((head, ""), |(i, _)| (i, &head[i.len()..]));
    let ids: Vec<String> = ids
        .split(',')
        .map(|id| {
            let name = id.trim();
            match aliases.get(name) {
                Some(new) => id.replacen(name, new, 1),
                None => id.to_owned(),
            }
        })
        .collect();
    format!("{}{reason}{tail}", ids.join(","))
}

/// The byte ranges of the comments of `text`, in order.
fn comments(text: &str, family: Family) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let here = bytes[at];
        let next = bytes.get(at + 1).copied();
        match (family, here, next) {
            (Family::Hash, b'#', _) => {
                let end = line_end(bytes, at);
                found.push(at..end);
                at = end;
            }
            (Family::Hash, b'"' | b'\'', _) => at = quoted(bytes, at, true),
            (Family::Hash, _, _) => at += 1,
            (_, b'/', Some(b'/')) => {
                let end = line_end(bytes, at);
                found.push(at..end);
                at = end;
            }
            (_, b'/', Some(b'*')) => {
                let end = block_end(bytes, at, family == Family::Rust);
                found.push(at..end);
                at = end;
            }
            (Family::Go | Family::CLike, b'`', _) => at = raw_backtick(bytes, at),
            (Family::Rust, b'r' | b'b', _) if rust_raw_start(bytes, at).is_some() => {
                at = rust_raw_end(bytes, at);
            }
            (Family::Rust, b'\'', _) => at = rust_quote(bytes, at),
            (Family::Go, b'\'', _) => at = quoted(bytes, at, false),
            (_, b'"', _) => at = quoted(bytes, at, family != Family::Go),
            (Family::CLike, b'\'', _) => at = quoted(bytes, at, false),
            _ => at += 1,
        }
    }
    found
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(bytes.len(), |n| from + n)
}

/// The end of the block comment starting at `from`; `nested` for languages
/// whose block comments nest.
fn block_end(bytes: &[u8], from: usize, nested: bool) -> usize {
    let mut depth = 0;
    let mut at = from;
    while at < bytes.len() {
        match (bytes[at], bytes.get(at + 1)) {
            (b'/', Some(b'*')) => {
                depth += 1;
                at += 2;
            }
            (b'*', Some(b'/')) => {
                depth -= 1;
                at += 2;
                if depth == 0 || !nested {
                    return at;
                }
            }
            _ => at += 1,
        }
    }
    bytes.len()
}

/// The end of the literal that starts with the quote at `from`: after the
/// closing quote, at the end of the line for a quote that never closes on it
/// (`multiline` literals run on), and past triple quotes when they start it.
fn quoted(bytes: &[u8], from: usize, multiline: bool) -> usize {
    let quote = bytes[from];
    let triple = bytes[from..].starts_with(&[quote, quote, quote]);
    let mut at = from + if triple { 3 } else { 1 };
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b if b == quote => {
                if triple {
                    if bytes[at..].starts_with(&[quote, quote, quote]) {
                        return at + 3;
                    }
                    at += 1;
                } else {
                    return at + 1;
                }
            }
            b'\n' if !multiline && !triple => return at,
            _ => at += 1,
        }
    }
    bytes.len()
}

fn raw_backtick(bytes: &[u8], from: usize) -> usize {
    bytes[from + 1..]
        .iter()
        .position(|b| *b == b'`')
        .map_or(bytes.len(), |n| from + n + 2)
}

/// `Some(hashes)` when a Rust raw string (`r"`, `r#"`, `br##"`) starts at
/// `from` and is not the tail of a longer word.
fn rust_raw_start(bytes: &[u8], from: usize) -> Option<usize> {
    let word = from > 0 && (bytes[from - 1].is_ascii_alphanumeric() || bytes[from - 1] == b'_');
    if word {
        return None;
    }
    let mut at = from;
    if bytes[at] == b'b' {
        at += 1;
    }
    if bytes.get(at) != Some(&b'r') {
        return None;
    }
    at += 1;
    let hashes = bytes[at..].iter().take_while(|b| **b == b'#').count();
    (bytes.get(at + hashes) == Some(&b'"')).then_some(hashes)
}

fn rust_raw_end(bytes: &[u8], from: usize) -> usize {
    let hashes = rust_raw_start(bytes, from).unwrap_or(0);
    let open = bytes[from..]
        .iter()
        .position(|b| *b == b'"')
        .map_or(bytes.len(), |n| from + n + 1);
    let mut close = vec![b'"'];
    close.extend(std::iter::repeat_n(b'#', hashes));
    (open..bytes.len())
        .find(|at| bytes[*at..].starts_with(&close))
        .map_or(bytes.len(), |at| at + close.len())
}

/// A `'` in Rust starts a character literal or a lifetime; only the literal
/// can hide a `//`.
fn rust_quote(bytes: &[u8], from: usize) -> usize {
    match (bytes.get(from + 1), bytes.get(from + 2)) {
        (Some(b'\\'), _) => bytes[from + 2..]
            .iter()
            .position(|b| *b == b'\'')
            .map_or(bytes.len(), |n| from + n + 3),
        (Some(c), Some(b'\'')) if *c != b'\n' => from + 3,
        _ => from + 1,
    }
}
