//! The fix of a finding as reports show it: SARIF replacements and, for the
//! agent formats, a unified diff or a one-line summary.

use lighthouse_model::{Position, Safety};
use serde_json::{Value, json};
use similar::TextDiff;

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

/// One file a fix changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixFile {
    pub path: String,
    pub before: String,
    pub after: String,
    /// The replacements, as spans of `before`.
    pub edits: Vec<FixEdit>,
}

/// Replaces the text between `start` and `end` (an insertion when equal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixEdit {
    pub start: Position,
    pub end: Position,
    pub text: String,
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
    pub(crate) fn of(fix: &Fix) -> Self {
        let diff = diff(fix);
        let body = if diff.lines().count() <= DIFF_LINES {
            Body::Diff(diff)
        } else {
            let changed = diff
                .lines()
                .filter(|l| {
                    l.starts_with(['+', '-']) && !l.starts_with("+++") && !l.starts_with("---")
                })
                .count();
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

/// The unified diff of a fix with one line of context; hunks only, with `---`
/// and `+++` headers when the fix spans several files. No trailing newline.
pub(crate) fn diff(fix: &Fix) -> String {
    let headers = fix.files.len() > 1;
    let mut out = String::new();
    for file in &fix.files {
        if headers {
            out.push_str(&format!("--- a/{0}\n+++ b/{0}\n", file.path));
        }
        let hunks = TextDiff::from_lines(&file.before, &file.after)
            .unified_diff()
            .context_radius(CONTEXT)
            .to_string();
        out.push_str(&hunks);
    }
    out.trim_end().to_owned()
}
