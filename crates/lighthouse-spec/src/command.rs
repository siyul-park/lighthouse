//! The `command` check: an external program under the process contract, the
//! options of how its output is read and the problems a spec can have.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The levels SARIF defines for a result.
const LEVELS: [&str; 4] = ["error", "warning", "note", "none"];

/// How a `command` check batches the files it is given.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Batch {
    /// One run per file.
    #[default]
    File,
    /// Runs over many files at once, as many per run as the argument list
    /// allows; `{files}` expands to several arguments.
    All,
}

/// What a `command` check gives the program on stdin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CheckStdin {
    #[default]
    None,
    /// The content of the file (only with `batch: file`).
    File,
}

/// Which exit codes mean what; every other code, a signal or a timeout is an
/// error and leaves the analysis incomplete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExitCodes {
    #[serde(default = "default_clean")]
    pub clean: Vec<i32>,
    #[serde(default = "default_findings")]
    pub findings: Vec<i32>,
}

impl Default for ExitCodes {
    fn default() -> Self {
        Self {
            clean: default_clean(),
            findings: default_findings(),
        }
    }
}

/// An external program that checks files, by the process contract: exit `0`
/// ran and found nothing, `1` ran and printed findings on stdout (one per
/// line; a leading `path:line[:col]: ` places it, else it attaches to the file
/// or the project), anything else (`>= 2`, a signal, a timeout) is an error
/// that leaves the analysis incomplete, never clean. stderr is for people.
/// It runs without a shell in the project root, read-only by contract, with an
/// environment cleared to `PATH`, `LANG`, `TMPDIR`, the declared
/// `env` and `LIGHTHOUSE_*`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CommandCheck {
    /// Program and arguments; `{file}`, `{files}` and `{rule}` fill whole
    /// arguments.
    pub argv: Vec<String>,
    #[serde(default)]
    pub batch: Batch,
    #[serde(default)]
    pub stdin: CheckStdin,
    /// Extra environment variables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "is_default_codes")]
    pub exit_codes: ExitCodes,
    /// How stdout is read when the program found something: one finding per
    /// line (default), or a SARIF 2.1.0 log.
    #[serde(default, skip_serializing_if = "CheckOutput::is_lines")]
    pub output: CheckOutput,
    /// Which SARIF results become findings; only with `output: sarif`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<SarifSelect>,
}

/// How a `command` check reads what the program printed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CheckOutput {
    /// One finding per line, with an optional `path:line[:col]: ` prefix.
    #[default]
    Lines,
    /// A SARIF 2.1.0 log: one finding per result.
    Sarif,
}

impl CheckOutput {
    fn is_lines(&self) -> bool {
        *self == Self::Lines
    }
}

/// The results of a SARIF log that become findings; a result must pass both
/// lists. This `select` is a field of the command check, not the CEL `select`
/// of other checks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SarifSelect {
    /// Globs over the result's `ruleId`, such as `errcheck` or `govet:*`: `*`
    /// is any run of characters inside one segment, segments being separated
    /// by `/` or `::`, and `**` is any number of segments, as in module path
    /// globs. Default: every rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_ids: Vec<String>,
    /// SARIF `level`s to keep: `error`, `warning`, `note` or `none`; a result
    /// without a level is a `warning`. Default: every level.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<String>,
}

impl CommandCheck {
    /// The first problem with the command, described.
    pub(crate) fn problem(&self) -> Option<String> {
        const KNOWN: [&str; 3] = ["{file}", "{files}", "{rule}"];
        if self.argv.first().is_none_or(|p| p.trim().is_empty()) {
            return Some("check: `argv` needs a program".to_owned());
        }
        if KNOWN.contains(&self.argv[0].as_str()) {
            return Some("check: the program cannot be a placeholder".to_owned());
        }
        if let Some(arg) = self
            .argv
            .iter()
            .find(|a| !KNOWN.contains(&a.as_str()) && (a.contains('{') && a.contains('}')))
        {
            return Some(format!(
                "check: `{arg}` is not a placeholder of its own: `{{file}}`, `{{files}}` and `{{rule}}` fill a whole argument"
            ));
        }
        let uses = |p: &str| self.argv.iter().any(|a| a == p);
        if uses("{files}") && self.batch == Batch::File {
            return Some("check: `{files}` needs `batch: all`".to_owned());
        }
        if uses("{file}") && self.batch == Batch::All {
            return Some(
                "check: `{file}` needs `batch: file`; use `{files}` with `batch: all`".to_owned(),
            );
        }
        if self.stdin == CheckStdin::File && self.batch == Batch::All {
            return Some("check: `stdin: file` needs `batch: file`".to_owned());
        }
        if self.select.is_some() && self.output != CheckOutput::Sarif {
            return Some("check: `select` needs `output: sarif`".to_owned());
        }
        let bad_level = self
            .select
            .iter()
            .flat_map(|s| &s.levels)
            .find(|l| !LEVELS.contains(&l.as_str()));
        if let Some(level) = bad_level {
            return Some(format!(
                "check: `select.levels` has `{level}`; SARIF levels are error, warning, note and none"
            ));
        }
        let overlap = self
            .exit_codes
            .clean
            .iter()
            .any(|c| self.exit_codes.findings.contains(c));
        if overlap || self.exit_codes.clean.is_empty() {
            return Some(
                "check: `exitCodes` needs at least one clean code and no code in both lists"
                    .to_owned(),
            );
        }
        None
    }
}

fn default_clean() -> Vec<i32> {
    vec![0]
}

fn default_findings() -> Vec<i32> {
    vec![1]
}

fn is_default_codes(codes: &ExitCodes) -> bool {
    *codes == ExitCodes::default()
}
