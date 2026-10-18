//! What a project says about fixes: which decisions a model may fix, how far
//! a change may go, whether a fix that is not safe is written without being
//! asked for, and the programs that must pass after the fixes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{FixScope, Point, Program, ProgramSpec};

/// What a project allows of fixes, as it resolves: the `fix` table with its
/// defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixPolicy {
    /// Whether a fix that is not safe, once verified, is written without
    /// `--unsafe-fixes`.
    pub unsafe_fixes: bool,
    /// The decisions a model may fix: ids or globs over them; none when the
    /// project lists none.
    pub decisions: Vec<String>,
    /// The widest change any fix by a model may make.
    pub scope: FixScope,
    /// The programs that must pass with the fixes by models in place.
    pub verify: Vec<Program>,
}

impl FixPolicy {
    /// The policy of the `fix` table, with the defaults.
    pub(super) fn new(spec: FixSpec) -> Self {
        Self {
            unsafe_fixes: spec.unsafe_fixes.unwrap_or_default(),
            decisions: spec.decisions,
            scope: spec.scope.unwrap_or(FixScope::Files),
            verify: spec.verify.into_iter().map(Program::from).collect(),
        }
    }
}

/// The `fix` table of a project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixSpec {
    /// Whether a fix that is not safe (one by a model among them) is written
    /// when its result is verified, as `--unsafe-fixes` does for one run;
    /// otherwise it is reported with its diff and left for a hand to apply.
    /// A user's file may set it. `--no-unsafe-fixes` turns it off for a run.
    /// Default: `false`.
    #[serde(rename = "unsafe", default, skip_serializing_if = "Option::is_none")]
    pub unsafe_fixes: Option<bool>,
    /// The decisions a model may fix, as ids or globs over them (`*` within
    /// one segment of an id): `["design/exported-doc", "testing/*"]`. Only a
    /// decision with a check and no fix of its own (`ops`, `command`) is
    /// fixed, and only by the agent `[models] agent` binds; binding a model
    /// enables nothing by itself. Default: none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    /// The widest change a fix by a model may make, for every decision. A
    /// decision that declares no `scope` has this one; one that declares a
    /// `scope` is narrowed to it, never widened. Default: `files`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<FixScope>,
    /// Programs that must pass over the project with the fixes by models in
    /// place, such as its tests: a command line, or a table with a `command`,
    /// a `timeout` (default `10m`) and the rest of a program. They run once
    /// per fix run, after its rounds, on a scratch copy of the project, and
    /// only in a trusted project. A program that fails (its `output` is
    /// `status` unless it says `lines`) withdraws every fix by a model of the
    /// run to a preview.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verify: Vec<ProgramSpec>,
}

impl FixSpec {
    /// Whether the project says nothing about fixing.
    pub fn is_empty(&self) -> bool {
        self.unsafe_fixes.is_none()
            && self.decisions.is_empty()
            && self.scope.is_none()
            && self.verify.is_empty()
    }

    /// The first problem with a verification program, named by its place.
    pub(super) fn problem(&self) -> Option<String> {
        self.verify.iter().enumerate().find_map(|(at, spec)| {
            Program::from(spec.clone())
                .problem(Point::Verify)
                .map(|why| format!("`fix.verify[{at}]`: {why}"))
        })
    }
}

/// The `fix` table of a user's or repository-local file: the one key of it
/// that is about this machine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixEnvironment {
    /// Whether a fix that is not safe is written once verified, as
    /// `--unsafe-fixes` does for one run.
    #[serde(rename = "unsafe", default, skip_serializing_if = "Option::is_none")]
    pub unsafe_fixes: Option<bool>,
}

impl FixEnvironment {
    /// Whether the layer says nothing about fixing.
    pub fn is_empty(&self) -> bool {
        self.unsafe_fixes.is_none()
    }
}
