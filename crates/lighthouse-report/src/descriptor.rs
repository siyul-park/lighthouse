//! The descriptor of a decision: how its check and its fix run, whatever the
//! provider, in one shape. Every report that tells a person or an agent what
//! to do with a finding shows it: the decision list, a group of findings, the
//! brief of a finding. The session builds it from the decision, the project
//! and what this machine can run; this module is its shape and the words it
//! is told in.

use lighthouse_model::{Safety, Unavailable};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

/// What decides the findings of a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckBy {
    Cel,
    Command,
    Model,
}

/// What makes the fix of a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FixBy {
    Cel,
    Command,
    Model,
    Rpc,
}

/// How the check of a decision runs here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CheckInfo {
    pub by: CheckBy,
    /// False when the check cannot run in this project, such as a command in
    /// a project nobody trusted.
    pub runs: bool,
}

/// A tool call: the twin of a command line.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ToolCall {
    pub name: &'static str,
    pub arguments: Value,
}

/// How a fix is applied, said both ways: the command line and the tool call.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Apply {
    /// The command line.
    pub apply: String,
    pub tool: ToolCall,
}

/// What an application of a fix is aimed at.
#[derive(Debug, Clone, Copy)]
pub enum Aim<'a> {
    /// Every finding of a decision.
    Decision(&'a str),
    /// Findings, by their fingerprints or prefixes of at least 7 characters.
    Findings(&'a [&'a str]),
}

impl Apply {
    /// The application of the fixes `aim` names; `unsafe_fixes` asks for
    /// permission to write the `unsafe` ones, as applying their previews
    /// does. Without it the `unsafe` ones are previewed, not written.
    pub fn of(aim: Aim<'_>, unsafe_fixes: bool) -> Self {
        let (selector, mut arguments) = match aim {
            Aim::Decision(id) => (format!("--decision {id}"), json!({ "decision": [id] })),
            Aim::Findings(fingerprints) => (
                fingerprints
                    .iter()
                    .map(|fingerprint| format!("--fingerprint {fingerprint}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                json!({ "fingerprints": fingerprints }),
            ),
        };
        let permission = if unsafe_fixes {
            arguments["unsafeFixes"] = json!(true);
            " --unsafe-fixes"
        } else {
            ""
        };
        Self {
            apply: format!("lighthouse check --fix{permission} {selector}"),
            tool: ToolCall {
                name: "fix",
                arguments,
            },
        }
    }

    /// The tool call as a line: its name and arguments.
    pub fn tool_line(&self) -> String {
        format!("{} {}", self.tool.name, self.tool.arguments)
    }
}

/// How the fix of a decision runs here.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct FixInfo {
    pub by: FixBy,
    /// The effective safety of the fix at best: a provider can only lower it.
    pub safety: Safety,
    /// False when the fix cannot run, and `why` says why.
    pub runs: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<Unavailable>,
    /// Whether an `unsafe` fix run without `--unsafe-fixes` is written
    /// (`[fix] unsafe`) or previewed. Absent for a `safe` fix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub written: Option<bool>,
    #[serde(flatten)]
    pub apply: Apply,
}

/// The check and the fix of a decision, and whether a finding of it has a
/// brief to fix it by hand.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Descriptor {
    pub check: CheckInfo,
    /// Absent when the decision has no fix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<FixInfo>,
    /// `review_show` gives a brief for a finding whose fix is absent, `unsafe`
    /// or not running: the way to fix it by hand.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub brief: bool,
}

impl Descriptor {
    /// The line an agent reads under a decision: what its fix is and how to
    /// apply it, else how to get the brief of `fingerprint`. `tool_form` names
    /// tool calls, not command lines.
    pub fn line(&self, fingerprint: &str, tool_form: bool) -> String {
        let brief = if tool_form {
            format!("review_show {fingerprint} for a brief")
        } else {
            format!("lighthouse review show {fingerprint} for a brief")
        };
        let Some(fix) = &self.fix else {
            return format!("fix: none \u{2014} {brief}");
        };
        let how = match (fix.why, fix.written) {
            (Some(why), _) => format!(", not running: {why}"),
            (None, Some(false)) => ", previewed; apply each preview by its fingerprint".to_owned(),
            (None, Some(true)) => ", written under `[fix] unsafe`".to_owned(),
            (None, None) => String::new(),
        };
        let next = match fix.why {
            Some(_) => brief,
            None if tool_form => fix.apply.tool_line(),
            None => fix.apply.apply.clone(),
        };
        format!(
            "fix: {} ({}{how}) \u{2014} {next}",
            fix.safety,
            fix.by.as_str()
        )
    }
}

impl CheckBy {
    /// The word the descriptor calls the provider of a check by.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cel => "cel",
            Self::Command => "command",
            Self::Model => "model",
        }
    }
}

impl FixBy {
    /// The word the descriptor and the agent text call the provider by.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cel => "cel",
            Self::Command => "command",
            Self::Model => "model",
            Self::Rpc => "rpc",
        }
    }
}
