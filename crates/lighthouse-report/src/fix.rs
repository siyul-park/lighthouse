//! The fix of a finding as reports show it: SARIF replacements and, for the
//! agent formats, a unified diff or a one-line summary.

use lighthouse_model::{Position, Safety};
use serde_json::{Value, json};
use similar::{ChangeTag, TextDiff};

/// Longest diff an agent finding carries inline, in lines; a longer fix is
/// summarized.
const DIFF_LINES: usize = 12;
/// Lines of unchanged text around a change in the diff.
const CONTEXT: usize = 1;

/// The fix proposed for a finding, computed without writing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub safety: Safety,
    pub description: String,
    /// The files it changes, sorted by path.
    pub files: Vec<FixFile>,
}

/// One file a fix changes. The texts are not kept: the unified hunks and the
/// replacements are computed once, when the preview is made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixFile {
    pub path: String,
    /// The unified diff hunks (`@@ -l,n +l,n @@`, one line of context), no
    /// file headers.
    hunks: String,
    /// Lines the hunks add or remove.
    changed: usize,
    /// The replacements, as spans of the original text in UTF-16 columns.
    pub edits: Vec<FixEdit>,
}

/// Replaces the text between `start` and `end` (an insertion when equal);
/// columns count UTF-16 code units, as SARIF regions do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixEdit {
    pub start: Position,
    pub end: Position,
    pub text: String,
}

impl FixFile {
    /// The change of one file from `before` to `after`, with `edits` as
    /// (start, end, text) in byte columns of `before`.
    pub fn new(
        path: String,
        before: &str,
        after: &str,
        edits: impl IntoIterator<Item = (Position, Position, String)>,
    ) -> Self {
        let diff = TextDiff::from_lines(before, after);
        let hunks = diff
            .unified_diff()
            .context_radius(CONTEXT)
            .to_string()
            .trim_end_matches('\n')
            .to_owned();
        let changed = diff
            .iter_all_changes()
            .filter(|c| c.tag() != ChangeTag::Equal)
            .count();
        let edits = edits
            .into_iter()
            .map(|(start, end, text)| FixEdit {
                start: utf16_position(before, start),
                end: utf16_position(before, end),
                text,
            })
            .collect();
        Self {
            path,
            hunks,
            changed,
            edits,
        }
    }
}

/// How an agent finding shows its fix: the diff, or a summary when the diff
/// is long.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shown {
    safety: Safety,
    body: Body,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Body {
    Diff(String),
    Summary(String),
}

impl Shown {
    /// How `fix`, proposed for a finding in `finding_file`, is shown. A file
    /// other than the finding's gets `---`/`+++` headers.
    pub(crate) fn of(fix: &Fix, finding_file: &str) -> Self {
        let diff = diff(fix, finding_file);
        let body = if diff.lines().count() <= DIFF_LINES {
            Body::Diff(diff)
        } else {
            let changed: usize = fix.files.iter().map(|f| f.changed).sum();
            Body::Summary(format!("{} ({changed} lines)", fix.description))
        };
        Self {
            safety: fix.safety,
            body,
        }
    }

    /// `{"safety", "diff"}`, or `{"safety", "summary"}` for a long fix.
    pub(crate) fn json(&self) -> Value {
        match &self.body {
            Body::Diff(diff) => json!({ "safety": self.safety.to_string(), "diff": diff }),
            Body::Summary(text) => json!({ "safety": self.safety.to_string(), "summary": text }),
        }
    }

    /// The lines to print under a finding, indented by `indent`.
    pub(crate) fn text(&self, indent: &str) -> Vec<String> {
        match &self.body {
            Body::Diff(diff) => std::iter::once(format!("{indent}fix [{}]", self.safety))
                .chain(diff.lines().map(|l| format!("{indent}  {l}")))
                .collect(),
            Body::Summary(text) => vec![format!("{indent}fix [{}]: {text}", self.safety)],
        }
    }
}

/// The unified diff of a fix: hunks, with `---` and `+++` headers for every file
/// when the fix touches a file other than `finding_file`. Trailing newlines are trimmed.
pub(crate) fn diff(fix: &Fix, finding_file: &str) -> String {
    let headers = fix.files.iter().any(|f| f.path != finding_file);
    let mut out = String::new();
    for file in &fix.files {
        if headers {
            out.push_str(&format!("--- a/{0}\n+++ b/{0}\n", file.path));
        }
        out.push_str(&file.hunks);
        out.push('\n');
    }
    out.trim_end_matches('\n').to_owned()
}

/// `at` with its byte column turned into a count of UTF-16 code units of the
/// line in `text`; a position outside the text is returned as it is.
pub(crate) fn utf16_position(text: &str, at: Position) -> Position {
    let line = text.split('\n').nth(at.line.saturating_sub(1) as usize);
    let Some(line) = line else { return at };
    let end = (at.col.saturating_sub(1) as usize).min(line.len());
    let col = line.get(..end).map_or(at.col, |head| {
        u32::try_from(head.encode_utf16().count()).map_or(at.col, |n| n + 1)
    });
    Position { col, ..at }
}
