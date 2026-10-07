use std::{fmt, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::{Node, Span, SymbolId};

/// How much a proposed fix may be trusted to keep the program's meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    /// The weaker of two claims: what a pattern's cap makes of a fixer's claim.
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
