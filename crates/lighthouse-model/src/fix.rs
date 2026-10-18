use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Fingerprint, Position};

/// How much a proposed fix may be trusted to keep the program's meaning.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    /// Mechanical and meaning-preserving: written by `--fix`.
    Safe,
    /// Probably right but a judgment call: written only when the project allows
    /// it or the run asks for unsafe fixes, else previewed.
    Unsafe,
}

vocabulary!(Safety, "safety" {
    Safe => "safe",
    Unsafe => "unsafe",
});

/// Why the fix of a decision does not run, whoever would make it: one of a
/// closed set, said the same way for a rule, a command, a plugin and a model.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum Unavailable {
    /// A model fix, and no agent that edits is bound to the decision.
    #[serde(rename = "no agent binding")]
    NoAgentBinding,
    /// The fix runs a program, and the user has not trusted the project.
    #[serde(rename = "untrusted project")]
    UntrustedProject,
    /// The fix runs a program, and this machine has no sandbox to run it in.
    #[serde(rename = "sandbox unavailable")]
    SandboxUnavailable,
    /// A model that costs, and `[models] budget` allows no request.
    #[serde(rename = "no budget")]
    NoBudget,
    /// A model that costs, and the requests the budget allows were made.
    #[serde(rename = "budget spent")]
    BudgetSpent,
    /// Nothing serves the kind of fix the decision names.
    #[serde(rename = "provider unavailable")]
    ProviderUnavailable,
}

vocabulary!(Unavailable, "reason" {
    NoAgentBinding => "no agent binding",
    UntrustedProject => "untrusted project",
    SandboxUnavailable => "sandbox unavailable",
    NoBudget => "no budget",
    BudgetSpent => "budget spent",
    ProviderUnavailable => "provider unavailable",
});

/// The fix proposed for one finding, computed without writing anything: what
/// reports show of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub fingerprint: Fingerprint,
    pub description: String,
    /// The effective safety: `Unsafe` when writing the fix needs permission.
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
