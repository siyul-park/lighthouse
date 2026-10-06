use std::collections::BTreeMap;

use cel::Program;
use lighthouse_plugin::Scope;
use serde::Deserialize;

use crate::Error;

/// What a rule looks at; each choice binds one variable of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Select {
    Symbol,
    Function,
    Edge,
    Module,
    File,
    Test,
}

impl Select {
    /// Edges and modules are judged once over the project; the rest per file.
    pub fn scope(self) -> Scope {
        match self {
            Self::Edge | Self::Module => Scope::Project,
            _ => Scope::File,
        }
    }

    /// The variable the selected value is bound to. `function` is reserved
    /// in CEL, so functions are `func`.
    pub fn variable(self) -> &'static str {
        match self {
            Self::Function => "func",
            other => other.name(),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Symbol => "symbol",
            Self::Function => "function",
            Self::Edge => "edge",
            Self::Module => "module",
            Self::File => "file",
            Self::Test => "test",
        }
    }
}

/// The text of a rule file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub select: Select,
    /// CEL; true for a violation.
    #[serde(rename = "where")]
    pub condition: String,
    /// Text with `{{ cel }}` holes.
    pub message: String,
    /// Evidence fields, each a CEL expression.
    #[serde(default)]
    pub evidence: BTreeMap<String, String>,
}

/// A rule file with every expression compiled.
#[derive(Debug)]
pub(crate) struct Compiled {
    pub select: Select,
    pub condition: Program,
    pub message: Vec<Piece>,
    pub evidence: Vec<(String, Program)>,
}

/// A piece of a message template.
#[derive(Debug)]
pub(crate) enum Piece {
    Text(String),
    Hole(Program),
}

impl Definition {
    pub(crate) fn compile(&self, rule: &str) -> Result<Compiled, Error> {
        let program = |what: &str, source: &str| {
            Program::compile(source).map_err(|e| Error::invalid(rule, format!("{what}: {e}")))
        };
        let mut evidence = Vec::new();
        for (name, source) in &self.evidence {
            evidence.push((
                name.clone(),
                program(&format!("evidence `{name}`"), source)?,
            ));
        }
        Ok(Compiled {
            select: self.select,
            condition: program("where", &self.condition)?,
            message: template(rule, &self.message)?,
            evidence,
        })
    }
}

fn template(rule: &str, text: &str) -> Result<Vec<Piece>, Error> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let (before, after) = rest.split_at(open);
        if !before.is_empty() {
            pieces.push(Piece::Text(before.to_owned()));
        }
        let Some(close) = after.find("}}") else {
            return Err(Error::invalid(rule, "message: `{{` is never closed"));
        };
        let source = after[2..close].trim();
        let program = Program::compile(source)
            .map_err(|e| Error::invalid(rule, format!("message `{source}`: {e}")))?;
        pieces.push(Piece::Hole(program));
        rest = &after[close + 2..];
    }
    if !rest.is_empty() {
        pieces.push(Piece::Text(rest.to_owned()));
    }
    Ok(pieces)
}
