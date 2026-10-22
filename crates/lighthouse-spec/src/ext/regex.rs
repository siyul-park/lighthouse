//! The regex extension of cel-go (`ext.Regex()`): `regex.replace`,
//! `regex.extract` and `regex.extractAll`, with RE2 patterns (the `regex`
//! crate has no look-around either, so a pattern means the same).
//!
//! - `regex.replace(target, pattern, replacement[, count])` replaces the
//!   non-overlapping matches, the first `count` of them when `count` is not
//!   negative. The replacement refers to a group of the pattern as `\1` to
//!   `\9` (`\0` is the whole match) and writes a backslash as `\\`; anything
//!   else after a backslash, a group the pattern lacks and a `$` are errors.
//! - `regex.extract(target, pattern)` is the first match, as an optional.
//! - `regex.extractAll(target, pattern)` is the list of all matches.
//!
//! A pattern with one capture group matches as that group, a pattern with more
//! is an error.

use cel::{
    DeclarationError, Env, ExecutionError,
    common::{
        types::{CelInt, CelList, CelOptional, CelString},
        value::Val,
    },
};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock, PoisonError},
};

use regex::{Captures, Regex};

/// One piece of a replacement string.
enum Piece {
    Text(String),
    Group(usize),
}

/// Registers the overloads on `env`.
pub(crate) fn install(env: &mut Env) -> Result<(), DeclarationError> {
    cel::add_overload!(env, fn replace: (CelString, CelString, CelString) -> Result<CelString>,
        name = "regex.replace")?;
    cel::add_overload!(env, fn replace_n: (CelString, CelString, CelString, CelInt) -> Result<CelString>,
        name = "regex.replace")?;
    cel::add_overload!(env, fn extract: (CelString, CelString) -> Result<CelOptional>,
        name = "regex.extract")?;
    cel::add_overload!(env, fn extract_all: (CelString, CelString) -> Result<CelList>,
        name = "regex.extractAll")
}

fn replace(
    target: &CelString<'_>,
    pattern: &CelString<'_>,
    replacement: &CelString<'_>,
) -> Result<CelString<'static>, ExecutionError> {
    substitute(target, pattern, replacement, -1)
}

fn replace_n(
    target: &CelString<'_>,
    pattern: &CelString<'_>,
    replacement: &CelString<'_>,
    count: &CelInt,
) -> Result<CelString<'static>, ExecutionError> {
    substitute(target, pattern, replacement, *count.inner())
}

fn substitute(
    target: &CelString<'_>,
    pattern: &CelString<'_>,
    replacement: &CelString<'_>,
    count: i64,
) -> Result<CelString<'static>, ExecutionError> {
    let function = "regex.replace";
    let regex = compile(function, pattern)?;
    let pieces = pieces(replacement.inner(), regex.captures_len() - 1)
        .map_err(|why| ExecutionError::function_error(function, why))?;
    let limit = usize::try_from(count).ok();
    let mut out = String::new();
    let mut at = 0;
    for (n, found) in regex.captures_iter(target.inner()).enumerate() {
        if limit.is_some_and(|limit| n >= limit) {
            break;
        }
        let whole = found.get(0).map_or(0..0, |m| m.range());
        out.push_str(&target.inner()[at..whole.start]);
        expand(&mut out, &pieces, &found);
        at = whole.end;
    }
    out.push_str(&target.inner()[at..]);
    Ok(out.into())
}

/// The pieces of `replacement`, or why it is not one.
fn pieces(replacement: &str, groups: usize) -> Result<Vec<Piece>, String> {
    let invalid = |why: &str| format!("invalid replace string: {why}");
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut chars = replacement.chars();
    while let Some(c) = chars.next() {
        match c {
            '$' => return Err(invalid("a `$` is not a group, write `\\N`")),
            '\\' => match chars.next() {
                Some('\\') => text.push('\\'),
                Some(d) if d.is_ascii_digit() => {
                    let group = d as usize - '0' as usize;
                    if group > groups {
                        return Err(invalid(&format!("the pattern has no group {group}")));
                    }
                    if !text.is_empty() {
                        pieces.push(Piece::Text(std::mem::take(&mut text)));
                    }
                    pieces.push(Piece::Group(group));
                }
                _ => return Err(invalid("a backslash is followed by a digit or a backslash")),
            },
            c => text.push(c),
        }
    }
    if !text.is_empty() {
        pieces.push(Piece::Text(text));
    }
    Ok(pieces)
}

fn expand(out: &mut String, pieces: &[Piece], found: &Captures<'_>) {
    for piece in pieces {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Group(group) => out.push_str(found.get(*group).map_or("", |m| m.as_str())),
        }
    }
}

fn extract(
    target: &CelString<'_>,
    pattern: &CelString<'_>,
) -> Result<CelOptional<'static>, ExecutionError> {
    let function = "regex.extract";
    let regex = compile(function, pattern)?;
    single(function, &regex)?;
    Ok(match regex.captures(target.inner()) {
        Some(found) => CelOptional::of(Box::new(CelString::from(
            matched(&regex, &found).to_owned(),
        ))),
        None => CelOptional::none(),
    })
}

fn extract_all(
    target: &CelString<'_>,
    pattern: &CelString<'_>,
) -> Result<CelList<'static>, ExecutionError> {
    let function = "regex.extractAll";
    let regex = compile(function, pattern)?;
    single(function, &regex)?;
    let all: Vec<Box<dyn Val>> = regex
        .captures_iter(target.inner())
        .map(|found| Box::new(CelString::from(matched(&regex, &found).to_owned())) as Box<dyn Val>)
        .collect();
    Ok(CelList::from(all))
}

/// Whether `regex` can extract: it has at most one capture group.
fn single(function: &str, regex: &Regex) -> Result<(), ExecutionError> {
    if regex.captures_len() > 2 {
        return Err(ExecutionError::function_error(
            function,
            "multiple capture groups",
        ));
    }
    Ok(())
}

/// The text of a match: the capture group when the pattern has one, else the
/// whole match.
fn matched<'t>(regex: &Regex, found: &Captures<'t>) -> &'t str {
    let group = usize::from(regex.captures_len() > 1);
    found.get(group).map_or("", |m| m.as_str())
}

fn compile(function: &str, pattern: &CelString<'_>) -> Result<Regex, ExecutionError> {
    static COMPILED: OnceLock<Mutex<HashMap<String, Regex>>> = OnceLock::new();
    let cache = COMPILED.get_or_init(Mutex::default);
    let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(found) = cache.get(pattern.inner()) {
        return Ok(found.clone());
    }
    let regex = Regex::new(pattern.inner()).map_err(|e| {
        ExecutionError::function_error(function, format!("invalid regex string: {e}"))
    })?;
    cache.insert(pattern.inner().to_string(), regex.clone());
    Ok(regex)
}
