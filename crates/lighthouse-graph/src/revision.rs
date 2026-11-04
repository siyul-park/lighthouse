use std::fmt;

use serde::{Deserialize, Serialize};

/// A counter that moves every time an input changes. A node verified at a
/// revision is known to be right for the inputs of that revision.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Revision(pub u64);

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

/// How often an input is expected to change. A node's durability is the
/// lowest of what it read, and a node that only reads durable inputs is green
/// without a walk while none of them changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Durability {
    /// Workspace files and overlays.
    Volatile,
    /// Configuration and the project's decisions.
    Stable,
    /// The binary, plugins and the bundled catalog.
    Static,
}

impl Durability {
    pub(crate) const COUNT: usize = 3;

    pub(crate) fn index(self) -> usize {
        self as usize
    }

    pub(crate) fn from_index(index: i64) -> Self {
        match index {
            0 => Self::Volatile,
            1 => Self::Stable,
            _ => Self::Static,
        }
    }
}
