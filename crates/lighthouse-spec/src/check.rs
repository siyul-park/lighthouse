//! The `check:` block of a decision: how a violation is detected. One
//! provider kind is selected by `type`; each compiles into the single `Rule`
//! interface. Further kinds (`command`, `rpc`, ...) join the same union.

use std::collections::BTreeMap;

use cel::Program;
use lighthouse_plugin::Scope as RunScope;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a decision is checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Check {
    /// A rule registered by a bundled plugin.
    Builtin(BuiltinCheck),
    /// A CEL expression over the code model.
    Cel(CelCheck),
}

/// A rule implemented in Rust and registered by a bundled plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuiltinCheck {
    /// The id of the registered rule.
    pub id: String,
}

/// What a CEL check looks at; each choice binds one variable of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
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
    pub fn scope(self) -> RunScope {
        match self {
            Self::Edge | Self::Module => RunScope::Project,
            _ => RunScope::File,
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

    /// The spelling used in decision files and in messages.
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

/// A rule written as data: a `where` expression that is true for a violation
/// of the selected values, and a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CelCheck {
    pub select: Select,
    /// CEL; true for a violation.
    #[serde(rename = "where")]
    pub condition: String,
    /// Text with `{{ cel }}` holes.
    pub message: String,
    /// Evidence fields, each a CEL expression.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub evidence: BTreeMap<String, String>,
}

impl CelCheck {
    /// The first expression that does not compile, described.
    pub(crate) fn problem(&self) -> Option<String> {
        let compile = |what: &str, source: &str| {
            Program::compile(source)
                .err()
                .map(|e| format!("check {what}: {e}"))
        };
        if let Some(problem) = compile("where", &self.condition) {
            return Some(problem);
        }
        for (name, source) in &self.evidence {
            if let Some(problem) = compile(&format!("evidence `{name}`"), source) {
                return Some(problem);
            }
        }
        template_problem(&self.message).map(|p| format!("check message: {p}"))
    }
}

/// Why a `{{ cel }}` template is not well formed.
pub(crate) fn template_problem(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open..];
        let Some(close) = after.find("}}") else {
            return Some("`{{` is never closed".to_owned());
        };
        let source = after[2..close].trim();
        if let Err(e) = Program::compile(source) {
            return Some(format!("`{source}`: {e}"));
        }
        rest = &after[close + 2..];
    }
    None
}
