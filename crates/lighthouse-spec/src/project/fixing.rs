//! What a project says about fixes by models: whether a verified one is
//! written without asking, and the commands that must pass after them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether a fix a model proposed and the checks accepted is written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Approval {
    /// Reported with its diff and left for a hand to apply.
    #[default]
    Preview,
    /// Written like any other applied fix, without `--unsafe-fixes`.
    Verified,
}

/// The `approval` table of a project: what happens without a person's hand.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalSpec {
    /// What becomes of a verified fix by a model. Default: `preview`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<Approval>,
}

impl ApprovalSpec {
    /// Whether the project says nothing about approval.
    pub fn is_empty(&self) -> bool {
        self.fix.is_none()
    }
}

/// The `fix` table of a project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixSpec {
    /// Commands (argument lists, no shell) that must pass over the project
    /// with the fixes by models in place, such as its tests. They run once
    /// per fix run, after its rounds, on a scratch copy of the project, and
    /// only in a trusted project. If one fails, every fix by a model of the
    /// run is withdrawn to a preview.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verify: Vec<Vec<String>>,
}

impl FixSpec {
    /// Whether the project says nothing about fixing.
    pub fn is_empty(&self) -> bool {
        self.verify.is_empty()
    }
}
