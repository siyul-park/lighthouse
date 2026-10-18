//! The `command` check: a program (see [`crate::Program`]) whose result is
//! findings, and the files it is run for.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Point, Program, glob_set};

/// An external program that checks files, by the process contract. Its
/// `output` says what it prints (`lines`, `sarif` or `status`); with `lines`,
/// exit `0` ran and found nothing and `1` ran and printed findings, one per
/// line, anything else (a signal, a timeout, a code outside `success`) is an
/// error that leaves the analysis incomplete, never clean. stderr is for
/// people. It runs without a shell in the project root, read-only by
/// contract. `{file}` runs it once per file, `{files}` over as many files as
/// the argument list allows, neither once over the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandCheck {
    #[serde(flatten)]
    pub program: Program,
    /// Globs of project-relative paths the program is run for, such as
    /// `**/*.go`. Default: every file the decision's scope applies to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

impl CommandCheck {
    /// The first problem with the command, described.
    pub(crate) fn problem(&self) -> Option<String> {
        self.program
            .problem(Point::Check)
            .map(|why| format!("check: {why}"))
            .or_else(|| {
                glob_set(&self.files)
                    .err()
                    .map(|why| format!("check: `files`: {why}"))
            })
    }
}

/// The keys a `command` check may have besides the common ones.
pub(crate) fn fields() -> Vec<&'static str> {
    let mut own = crate::program::FIELDS.to_vec();
    own.push("files");
    own
}
