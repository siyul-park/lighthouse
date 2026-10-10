/// Rule id of the finding for a directive that gives no reason.
pub const ANNOTATION_REASON: &str = "core/annotation-reason";
/// Rule id of the finding for a directive that suppresses nothing.
pub const UNUSED_ALLOW: &str = "core/unused-allow";

const REASON_SEPARATOR: &str = "--";

/// The markers a directive starts with, longest first so that
/// `lighthouse-disable-next-line` is not read as `lighthouse-disable`.
const MARKERS: [(&str, Form); 5] = [
    ("lighthouse-disable-next-line", Form::NextLine),
    ("lighthouse-disable-line", Form::Line),
    ("lighthouse-disable", Form::Disable),
    ("lighthouse-enable", Form::Enable),
    (ALLOW, Form::NextLine),
];

/// The marker of the first form of the directive, kept as an alias of
/// `lighthouse-disable-next-line`.
const ALLOW: &str = "lighthouse:allow";

/// What a directive does, in the ESLint forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// `lighthouse-disable <id>[, <id>] -- <reason>`: from this comment to a
    /// matching `lighthouse-enable`, or to the end of the file; at the top of
    /// a file, the whole file.
    Disable,
    /// `lighthouse-enable <id>[, <id>]`: ends a range.
    Enable,
    /// `lighthouse-disable-next-line <id>[, <id>] -- <reason>`: the next line
    /// and the symbol declared there; `lighthouse:allow` is its alias.
    NextLine,
    /// `lighthouse-disable-line <id>[, <id>] -- <reason>`: this line.
    Line,
}

impl Form {
    /// Whether the form needs a reason: every form that disables.
    pub fn needs_reason(self) -> bool {
        self != Self::Enable
    }
}

/// A suppression directive: a comment line that starts with one of the
/// markers, after its comment markers. There is no form without ids: a
/// directive that names none suppresses nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    pub form: Form,
    /// The marker as it was written, such as `lighthouse-disable-next-line`.
    pub marker: &'static str,
    pub rules: Vec<String>,
    /// Why the finding is acceptable here; a directive that disables without
    /// one does not suppress anything.
    pub reason: Option<String>,
}

/// The first directive in a comment's text, if a line of it is one. Prose that
/// merely mentions a marker mid-line is not a directive.
pub fn parse(comment: &str) -> Option<Directive> {
    comment.lines().find_map(parse_line)
}

/// Every directive of a comment's text with the index of its line in the text:
/// adjacent line comments form one comment, and each line may hold one.
pub fn directives(comment: &str) -> Vec<(usize, Directive)> {
    comment
        .lines()
        .enumerate()
        .filter_map(|(at, line)| Some((at, parse_line(line)?)))
        .collect()
}

/// The index of the line of a comment's text that holds the first directive.
pub fn line_of(comment: &str) -> Option<usize> {
    comment.lines().position(|line| parse_line(line).is_some())
}

fn parse_line(line: &str) -> Option<Directive> {
    let line = line.trim_start_matches(|c: char| c.is_whitespace() || "/*#!".contains(c));
    let (marker, form, rest) = MARKERS.iter().find_map(|&(marker, form)| {
        let rest = line.strip_prefix(marker)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then_some((marker, form, rest))
    })?;
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
    Some(Directive {
        form,
        marker,
        rules,
        reason: reason.map(str::to_owned),
    })
}
