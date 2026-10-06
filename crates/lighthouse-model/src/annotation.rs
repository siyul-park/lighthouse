/// Rule id of the finding for an allow annotation that gives no reason.
pub const ANNOTATION_REASON: &str = "core/annotation-reason";
/// Rule id of the finding for an allow annotation that suppresses nothing.
pub const UNUSED_ALLOW: &str = "core/unused-allow";

/// What a comment line must start with, after its comment markers, to be an
/// allow annotation.
const MARKER: &str = "lighthouse:allow";
const REASON_SEPARATOR: &str = "--";

/// An allow annotation: `lighthouse:allow <rule>[, <rule>] -- <reason>` at the
/// start of a comment line. It suppresses the findings of the named rules on
/// the symbol or line the comment is attached to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allow {
    pub rules: Vec<String>,
    /// Why the finding is acceptable here; an annotation without one does not
    /// suppress anything.
    pub reason: Option<String>,
}

/// The allow annotation in a comment's text, if a line of it is one. Prose
/// that merely mentions the marker mid-line is not an annotation.
pub fn parse(comment: &str) -> Option<Allow> {
    comment.lines().find_map(parse_line)
}

fn parse_line(line: &str) -> Option<Allow> {
    let line = line.trim_start_matches(|c: char| c.is_whitespace() || "/*#!".contains(c));
    let rest = line.strip_prefix(MARKER)?;
    let rest = rest.trim_end().trim_end_matches("*/").trim_end();
    let (rules, reason) = match rest.split_once(&format!(" {REASON_SEPARATOR}")) {
        Some((rules, reason)) => (rules, Some(reason.trim()).filter(|r| !r.is_empty())),
        None => (rest, None),
    };
    let rules: Vec<String> = rules
        .split(',')
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_owned)
        .collect();
    (!rules.is_empty()).then(|| Allow {
        rules,
        reason: reason.map(str::to_owned),
    })
}
