//! What an operation hands back to its frontend, and who is calling.

use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use lighthouse_session::{
    AgentKind, Attribution,
    review::{attribute_to, person},
};
use serde::Serialize;
use serde_json::Value;

use crate::surface::Result;

/// The exit code of a run that found nothing wrong.
pub const OK: u8 = 0;
/// The exit code of a run that found something: findings over the threshold, a
/// failed test or validation, stale docs.
pub const FOUND: u8 = 1;
/// The exit code of a run whose arguments or environment were wrong.
pub const USAGE: u8 = 2;
/// The exit code of a run whose analysis was incomplete.
pub const INCOMPLETE: u8 = 3;

/// How a command encodes the data it prints.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Output {
    /// The human form.
    #[default]
    Text,
    /// One JSON document on one line.
    Json,
}

/// `--format` of the commands that print data.
#[derive(Debug, Default, Clone, Copy, Args)]
pub struct OutputArg {
    /// `text` for people, `json` for programs (its schema: `lighthouse schema`).
    #[arg(long, value_enum, default_value_t)]
    pub format: Output,
}

/// `--format` of `check`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CheckFormat {
    /// `path:line:col: severity id: message`, one finding a line.
    #[default]
    Text,
    /// Findings grouped by decision, as one JSON document on one line.
    Json,
    /// SARIF 2.1.0.
    Sarif,
}

/// Who is calling: a person at a terminal, or an agent through MCP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    Person,
    /// A coding agent, named by its client.
    Agent(String),
}

/// What an operation needs to know besides its arguments.
#[derive(Debug, Clone)]
pub struct Context {
    /// The project file, when `--config` names one.
    pub config: Option<PathBuf>,
    pub caller: Caller,
    /// Keep the caller inside the project: paths must lie under its root and a
    /// fix must name what it fixes. A person at a terminal is not confined.
    pub confine: bool,
}

impl Context {
    /// A person at a terminal.
    pub fn person(config: Option<PathBuf>) -> Self {
        Self {
            config,
            caller: Caller::Person,
            confine: false,
        }
    }

    /// A coding agent, named by its client.
    pub fn agent(name: impl Into<String>) -> Self {
        Self {
            config: None,
            caller: Caller::Agent(name.into()),
            confine: true,
        }
    }

    /// Who a record is attributed to when `--attributed-to` says: the
    /// default is the person, or the agent, that is calling.
    pub fn attribution(&self, root: &Path, spec: Option<&str>) -> Result<Attribution> {
        let default = match &self.caller {
            Caller::Person => person(root),
            Caller::Agent(name) => Attribution::new(AgentKind::SoftwareAgent, Some(name.clone())),
        };
        attribute_to(spec, &default)
    }

    /// The paths, canonical, each required to lie under the project root when
    /// the caller is confined.
    pub fn paths(&self, root: &Path, paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
        if !self.confine {
            return Ok(paths.to_vec());
        }
        let root = root.canonicalize()?;
        paths
            .iter()
            .map(|path| {
                let absolute = path
                    .canonicalize()
                    .map_err(|e| format!("`{}`: {e}", path.display()))?;
                if absolute.starts_with(&root) {
                    Ok(absolute)
                } else {
                    Err(format!(
                        "`{}` is outside the project root {}",
                        path.display(),
                        root.display()
                    )
                    .into())
                }
            })
            .collect()
    }
}

/// The result of an operation. A frontend prints `stdout` to stdout and
/// `notes` to stderr, and exits with `code`; MCP answers with `value`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Reply {
    /// The data in the form asked for, ending with a newline unless empty.
    pub stdout: String,
    /// The data as JSON: what `--format json` prints. `Null` for the forms
    /// that are not JSON.
    pub value: Value,
    /// What the user should know, one line each, for stderr.
    pub notes: Vec<String>,
    pub code: u8,
}

impl Reply {
    /// The reply of an operation whose data is `data`: its JSON on one line,
    /// or `text` called to make the human form.
    pub fn data<T: Serialize>(
        output: Output,
        data: &T,
        text: impl FnOnce() -> String,
    ) -> Result<Self> {
        let value = serde_json::to_value(data)?;
        Ok(match output {
            Output::Json => Self {
                stdout: line(&value)?,
                value,
                ..Self::default()
            },
            Output::Text => Self {
                stdout: text(),
                ..Self::default()
            },
        })
    }

    /// A reply that is JSON.
    pub fn json(value: Value) -> Result<Self> {
        Ok(Self {
            stdout: line(&value)?,
            value,
            ..Self::default()
        })
    }

    /// A reply that is text.
    pub fn text(stdout: String) -> Self {
        Self {
            stdout,
            ..Self::default()
        }
    }

    /// The same reply with an exit code.
    pub fn with_code(self, code: u8) -> Self {
        Self { code, ..self }
    }

    /// The same reply with notes for stderr.
    pub fn with_notes(self, notes: Vec<String>) -> Self {
        Self { notes, ..self }
    }
}

/// `value` on one line, ending with a newline.
fn line(value: &Value) -> Result<String> {
    let mut text = serde_json::to_string(value)?;
    text.push('\n');
    Ok(text)
}
