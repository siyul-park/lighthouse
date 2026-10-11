use std::{fmt, path::PathBuf, str::FromStr};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use similar::{ChangeTag, TextDiff};

use crate::{Fingerprint, Node, Position, Span, SymbolId, utf16_position};

/// Lines of unchanged text around a change in the hunks of a preview.
const CONTEXT: usize = 1;

/// How much a proposed fix may be trusted to keep the program's meaning.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    /// Mechanical and meaning-preserving: applied by default.
    Safe,
    /// Probably right but a judgment call: applied only when asked for.
    Suggested,
}

/// The text given to [`Safety::from_str`] names no safety.
#[derive(Debug, thiserror::Error)]
#[error("unknown safety `{0}` (expected safe or suggested)")]
pub struct UnknownSafety(pub String);

impl Safety {
    /// The weaker of two claims: what a decision's cap makes of a fixer's claim.
    pub fn capped(self, cap: Self) -> Self {
        self.max(cap)
    }
}

impl fmt::Display for Safety {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Safe => "safe",
            Self::Suggested => "suggested",
        })
    }
}

impl FromStr for Safety {
    type Err = UnknownSafety;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "safe" => Ok(Self::Safe),
            "suggested" => Ok(Self::Suggested),
            _ => Err(UnknownSafety(s.to_owned())),
        }
    }
}

/// Where a moved declaration lands, relative to another one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    Before(Node),
    After(Node),
}

/// The declarations an op reorders: those that share a container with its
/// members. Used to check that every member is in the named place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    File(PathBuf),
    Symbol(SymbolId),
}

/// An edit stated over code-model nodes, independent of any language. The
/// orchestrator lowers it to text edits with the spans providers supply:
/// there is no round-trip IR, so trivia and formatting stay as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditOp {
    /// Relocates a declaration with its extent.
    Move { node: Node, anchor: Anchor },
    /// Removes a declaration with its extent.
    Delete { node: Node },
    /// Puts the listed declarations, which share a container, in this order
    /// within the places they occupy.
    Reorder { owner: Owner, order: Vec<Node> },
    /// Renames a symbol at its declaration and at every reference site. Only
    /// when each reference edge is semantic and has a site.
    Rename { symbol: SymbolId, name: String },
    /// Removes a range of text, such as a comment.
    DeleteRange { file: PathBuf, span: Span },
    /// Replaces a range of text; an empty range inserts.
    Replace {
        file: PathBuf,
        span: Span,
        text: String,
    },
}

/// What a fixer answers for one finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum FixOutcome {
    Proposed {
        description: String,
        ops: Vec<EditOp>,
        safety: Safety,
    },
    /// The fixer cannot or will not fix this finding; never a guess.
    Declined { reason: String },
}

impl FixOutcome {
    /// A refusal with its reason.
    pub fn declined(reason: impl Into<String>) -> Self {
        Self::Declined {
            reason: reason.into(),
        }
    }
}

/// The fix proposed for one finding, computed without writing anything: what
/// reports show of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub fingerprint: Fingerprint,
    pub description: String,
    /// `Suggested` when applying the fix needs unsafe fixes, whatever the
    /// fixer claims: a suggested proposal, a suggested cap or a rule that is
    /// not mechanical.
    pub safety: Safety,
    /// The files it changes, sorted by path.
    pub files: Vec<PreviewFile>,
}

/// One file a fix changes. The texts are not kept: the unified hunks and the
/// replacements are computed once, when the preview is made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFile {
    pub path: String,
    /// The unified diff hunks (`@@ -l,n +l,n @@`, one line of context), no
    /// file headers.
    pub hunks: String,
    /// Lines the hunks add or remove.
    pub changed: usize,
    /// The replacements, as spans of the original text in UTF-16 columns.
    pub edits: Vec<PreviewEdit>,
}

/// Replaces the text between `start` and `end` (an insertion when equal);
/// columns count UTF-16 code units, as SARIF regions do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewEdit {
    pub start: Position,
    pub end: Position,
    pub text: String,
}

impl PreviewFile {
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
            .map(|(start, end, text)| PreviewEdit {
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
