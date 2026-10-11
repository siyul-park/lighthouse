//! The fix of a finding as reports show it: SARIF replacements and, for the
//! agent formats, a unified diff or a one-line summary.

use lighthouse_model::{Preview, Safety};
use serde_json::{Value, json};

/// Longest diff an agent finding carries inline, in lines; a longer fix is
/// summarized.
const DIFF_LINES: usize = 12;

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
    pub(crate) fn of(fix: &Preview, finding_file: &str) -> Self {
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
pub(crate) fn diff(fix: &Preview, finding_file: &str) -> String {
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
