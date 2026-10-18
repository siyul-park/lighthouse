//! What a project says about fixes by models: which decisions it allows, how
//! far a change may go, whether a verified one is written without asking, and
//! the commands that must pass after them.

use lighthouse_resource::Duration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::FixScope;

/// How long each command of `fix.verify` may run unless the project says.
pub(super) const DEFAULT_VERIFY_TIMEOUT: Duration = Duration::from_secs(600);

/// Whether a fix a model proposed and the checks accepted is written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Approval {
    /// Reported with its diff (previewed) and left for a hand to apply.
    #[default]
    Ask,
    /// Written (applied) like any other applied fix, without `--unsafe-fixes`.
    Allow,
}

/// The `approval` table of a project: what happens without a person's hand.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalSpec {
    /// What becomes of a verified fix by a model. Default: `ask`.
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
    /// The decisions a model may fix, as ids or globs over them (`*` within
    /// one segment of an id): `["design/exported-doc", "testing/*"]`. Only a
    /// decision with a check and no fix of its own (`ops`, `command`) is
    /// fixed, and only by the agent `[models] edit` binds; binding a model
    /// enables nothing by itself. Default: none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    /// The widest change a model's fix may make, for every decision. A
    /// decision that declares no `scope` has this one; one that declares a
    /// `scope` is narrowed to it, never widened. Default: `files`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<FixScope>,
    /// Commands (argument lists, no shell) that must pass over the project
    /// with the fixes by models in place, such as its tests. They run once
    /// per fix run, after its rounds, on a scratch copy of the project, and
    /// only in a trusted project. If one fails, every fix by a model of the
    /// run is withdrawn to a preview.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verify: Vec<Vec<String>>,
    /// How long each of those commands may run. Default: `10m`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
}

impl FixSpec {
    /// Whether the project says nothing about fixing.
    pub fn is_empty(&self) -> bool {
        self.decisions.is_empty()
            && self.scope.is_none()
            && self.verify.is_empty()
            && self.timeout.is_none()
    }
}
