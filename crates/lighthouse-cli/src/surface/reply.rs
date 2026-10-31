//! What an operation hands back to its frontend, and who is calling.

use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use lighthouse_session::{
    AgentKind, Attribution, Session, project_root,
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

/// `--format` of the commands that print data.
#[derive(Debug, Default, Clone, Copy, Args)]
pub struct FormatArg {
    /// `text` for people, `json` for programs (its schema: `lighthouse schema`).
    #[arg(long, value_enum, default_value_t)]
    pub format: Format,
}

/// How a command encodes the data it prints.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// The human form.
    #[default]
    Text,
    /// One JSON document on one line.
    Json,
}

/// `--format` of `check`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CheckFormat {
    /// `path:line:col: severity id: message`, one finding a line.
    #[default]
    Text,
    /// Findings grouped by decision, as one JSON document on one line.
    Json,
    /// SARIF 2.1.0. With `--fix`, the fixes the run left as previews, and in
    /// a dry run every fix it would make, are the `fixes` of their findings.
    Sarif,
}

/// What an operation needs to know besides its arguments.
#[derive(Debug, Clone)]
pub struct Context {
    /// The project file, when `--config` names one.
    pub config: Option<PathBuf>,
    pub caller: Caller,
}

impl Context {
    /// A person at a terminal.
    pub fn person(config: Option<PathBuf>) -> Self {
        Self {
            config,
            caller: Caller::Person,
        }
    }

    /// A coding agent, named by its client.
    pub fn agent(name: impl Into<String>) -> Self {
        Self {
            config: None,
            caller: Caller::Agent(name.into()),
        }
    }

    /// The project, or the default one where there is none.
    pub fn session(&self) -> Result<Session> {
        Session::load_or_default(self.config.as_deref())
    }

    /// The project root, without loading the project.
    pub fn root(&self) -> Result<PathBuf> {
        project_root(self.config.as_deref())
    }

    /// Whether the caller is kept inside the project: paths must lie under
    /// its root and a fix must name what it fixes. A person at a terminal is
    /// not confined.
    pub fn confined(&self) -> bool {
        matches!(self.caller, Caller::Agent(_))
    }

    /// Who a record is attributed to when `--attributed-to` says: the
    /// default is the person, or the agent, that is calling.
    pub fn attribution(&self, spec: Option<&str>) -> Result<Attribution> {
        let default = match &self.caller {
            Caller::Person => person(&self.root()?),
            Caller::Agent(name) => Attribution::new(AgentKind::SoftwareAgent, Some(name.clone())),
        };
        attribute_to(spec, &default)
    }

    /// The paths, canonical, each required to lie under the project root when
    /// the caller is confined.
    pub fn paths(&self, root: &Path, paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
        if !self.confined() {
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

/// Who is calling: a person at a terminal, or an agent through MCP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    Person,
    /// A coding agent, named by its client.
    Agent(String),
}

/// The result of an operation. A frontend prints the `body` to stdout and
/// the `notes` to stderr, and exits with `code`.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub body: Body,
    /// What the user should know, one line each, for stderr.
    pub notes: Vec<String>,
    pub code: u8,
}

impl Reply {
    /// The reply of an operation whose data is `data`: JSON, or `text` called
    /// to make the human form.
    pub fn data<T: Serialize>(
        format: Format,
        data: &T,
        text: impl FnOnce() -> String,
    ) -> Result<Self> {
        match format {
            Format::Json => Self::json(data),
            Format::Text => Ok(Self::text(text())),
        }
    }

    /// A reply that is JSON.
    pub fn json<T: Serialize>(data: &T) -> Result<Self> {
        Ok(Self {
            body: Body::Json(serde_json::to_value(data)?),
            notes: Vec::new(),
            code: OK,
        })
    }

    /// A reply that is text.
    pub fn text(text: String) -> Self {
        Self {
            body: Body::Text(text),
            notes: Vec::new(),
            code: OK,
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

    /// The same reply with notes for stderr that say `messages`, a line each.
    pub fn with_messages(self, messages: &[String]) -> Self {
        let notes = messages
            .iter()
            .map(|m| format!("lighthouse: {m}"))
            .collect();
        Self { notes, ..self }
    }
}

/// What an operation prints: text, or JSON that the terminal writes on one
/// line and MCP hands over as it is.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Text(String),
    Json(Value),
}
