use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Fingerprint, Position};

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

vocabulary!(Safety, "safety" {
    Safe => "safe",
    Suggested => "suggested",
});

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
