//! The `line_widths` of a file: how wide each line is, and whether it is one a
//! formatter cannot wrap.

use serde_json::{Value, json};

const COMMENT_MARKS: &[&str] = &["///", "//!", "//", "/*", "*/", "*", "#", "--"];
const IMPORTS: &[&str] = &["import ", "import(", "use ", "from "];

/// `[{ line, width, exempt }]` for every line of `text`; the width counts
/// Unicode scalars, a tab as one.
pub(super) fn line_widths(text: &str) -> Value {
    text.lines()
        .enumerate()
        .map(|(i, line)| {
            json!({
                "line": i + 1,
                "width": line.chars().count(),
                "exempt": exempt(line.trim()),
            })
        })
        .collect()
}

/// Whether a trimmed line is unbreakable: a comment holding one URL, a single
/// string literal, or an import.
fn exempt(line: &str) -> bool {
    if IMPORTS.iter().any(|p| line.starts_with(p)) {
        return true;
    }
    let body = COMMENT_MARKS
        .iter()
        .find_map(|m| line.strip_prefix(m))
        .map(|body| body.trim().trim_end_matches("*/").trim());
    if let Some(body) = body {
        return is_url(body);
    }
    let literal = line.strip_suffix([',', ';']).unwrap_or(line);
    ['"', '`'].iter().any(|q| {
        literal.len() > 1
            && literal.starts_with(*q)
            && literal.ends_with(*q)
            && !literal[1..literal.len() - 1].contains(*q)
    })
}

/// `^\S*://\S+$`.
fn is_url(body: &str) -> bool {
    !body.contains(char::is_whitespace)
        && body
            .split_once("://")
            .is_some_and(|(_, rest)| !rest.is_empty())
}
