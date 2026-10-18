//! The `fix:` block of a decision: how the finding of its rule is fixed. A
//! fix is data, never code of the rule: its `type` names how the proposal is
//! made (generic operations over the code model, an external command, a
//! model) and every type compiles into a provider of the one `Fixer`
//! interface.

use std::{collections::BTreeMap, collections::BTreeSet};

use lighthouse_model::{Safety, Severity, ucm::Capability};
use lighthouse_process::{Output, Stdin};
use lighthouse_resource::Duration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    DEFAULT_TIMEOUT, DecisionSpec, OrderScope,
    command::mixed_placeholder,
    expression::{compile_problem, template_problem},
    union::{refuse_strays, stray_field},
};

/// How a decision's findings are fixed and how far the fix may be trusted.
/// Which fields a fix may have depends on its `type`; any other is refused.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[schemars(transform = refuse_strays)]
pub struct Fix {
    /// Caps what the fixer may claim: a `safe` proposal under a `suggested`
    /// decision is downgraded. `safe` is reserved for decisions that author `error`.
    pub safety: Safety,
    /// Provider capabilities the fix needs; without them it is declined.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<Capability>,
    #[serde(flatten)]
    pub kind: FixKind,
}

/// The fields of a fix as it is read, before its `type` has had its say about
/// which of them may be there.
#[derive(Deserialize)]
struct Written {
    safety: Safety,
    #[serde(default)]
    requires: Vec<Capability>,
    #[serde(flatten)]
    kind: FixKind,
}

impl<'de> Deserialize<'de> for Fix {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        const COMMON: [&str; 3] = ["safety", "requires", "type"];
        let value = Value::deserialize(deserializer)?;
        let table = value
            .as_object()
            .ok_or_else(|| D::Error::custom("a fix is a table"))?;
        let own: &[&str] = match table.get("type").and_then(Value::as_str) {
            Some("ops") => &["ops"],
            Some("command") => &["argv", "output", "stdin", "env", "timeout"],
            Some("model") => &["prompt", "scope"],
            Some("rpc") => &["params"],
            other => {
                return Err(D::Error::custom(match other {
                    None => "a fix needs a `type`: ops, command, model or rpc".to_owned(),
                    Some(found) => {
                        format!("fix `type` is `{found}`, expected ops, command, model or rpc")
                    }
                }));
            }
        };
        if let Some(problem) = stray_field(table, "fix", &COMMON, own) {
            return Err(D::Error::custom(problem));
        }
        let Written {
            safety,
            requires,
            kind,
        } = serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(Self {
            safety,
            requires,
            kind,
        })
    }
}

impl Fix {
    /// What is wrong with the fix of `decision`, for the catalog to refuse.
    pub(crate) fn problem(&self, decision: &DecisionSpec) -> Option<String> {
        if decision.check.is_none() {
            return Some("a decision without a check has no findings to fix".to_owned());
        }
        if self.safety == Safety::Safe && decision.severity != Some(Severity::Error) {
            return Some(
                "`safe` is reserved for decisions that author `error`; use `suggested`".to_owned(),
            );
        }
        match &self.kind {
            FixKind::Ops { ops } if ops.is_empty() => Some("`ops` is empty".to_owned()),
            FixKind::Ops { ops } => {
                let renames = ops.iter().any(|op| matches!(op, OpSpec::Rename { .. }));
                if renames && !self.requires.contains(&Capability::CompleteReferences) {
                    return Some("`rename` needs `complete-references` in `requires`".to_owned());
                }
                ops.iter().find_map(OpSpec::problem)
            }
            FixKind::Command(command) => command.problem(),
            FixKind::Model { .. } if self.safety == Safety::Safe => {
                Some("a fix by a model is never `safe`; use `suggested`".to_owned())
            }
            FixKind::Model { .. } => None,
            // Kept, and declined when a fix is asked for; see `FixPlan`.
            FixKind::Rpc { .. } => None,
        }
    }
}

impl Fix {
    /// Whether the fix is made without a model, so that the same finding
    /// always gets the same proposal.
    pub fn deterministic(&self) -> bool {
        !matches!(self.kind, FixKind::Model { .. })
    }
}

/// How far the change of a fix by a model may go, which the verifier checks.
/// Ordered from the narrowest.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum FixScope {
    /// Only inside the symbol of the finding (its extent): the symbol it is
    /// about, else the innermost one around it, else a short file as a whole.
    #[default]
    Symbol,
    /// Any file the analysis indexes in the run's scope.
    Files,
}

impl FixScope {
    /// The scope of the fix by a model of a decision whose `fix` is `fix`
    /// (`symbol` unless it says more), narrowed to `cap`, the widest the
    /// project allows.
    pub fn of_fix(fix: Option<&Fix>, cap: Self) -> Self {
        let own = match fix.map(|f| &f.kind) {
            Some(FixKind::Model { scope, .. }) => scope.unwrap_or_default(),
            _ => Self::default(),
        };
        own.min(cap)
    }
}

impl std::fmt::Display for FixScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Symbol => "symbol",
            Self::Files => "files",
        })
    }
}

/// The type of a fix; a `fix:` block holds exactly one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FixKind {
    /// Generic operations over the code model, evaluated with CEL.
    Ops { ops: Vec<OpSpec> },
    /// An external program on a scratch copy of the files.
    Command(CommandSpec),
    /// An agent that edits fixes the finding, and the checks accept the
    /// result or not. The project lists the decisions this applies to
    /// (`[fix] decisions`); a listed decision without a fix of its own has
    /// this one with the defaults.
    Model {
        /// What the agent is told besides the decision's requirement, such
        /// as how this decision is usually fixed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
        /// How far the agent's change may go. Default: `symbol`. The
        /// project's `[fix] scope` narrows it and never widens it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<FixScope>,
    },
    /// A language plugin's fix method. Reserved: not yet supported.
    Rpc {
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        params: Map<String, Value>,
    },
}

/// One operation of an `ops` fix. Every parameter but `scope` and `by` is a
/// CEL expression over the finding (`finding`, `symbol`, `options`) or, for
/// `text` and `name`, a template with `{{ cel }}` holes. A `when` expression
/// that is false skips the operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum OpSpec {
    Move {
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<String>,
    },
    Reorder {
        scope: OrderScope,
        by: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<String>,
    },
    Delete {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        span: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<String>,
    },
    Rename {
        symbol: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<String>,
    },
    Replace {
        file: String,
        span: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<String>,
    },
}

impl OpSpec {
    /// The operation's name in a rule file.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Move { .. } => "move",
            Self::Reorder { .. } => "reorder",
            Self::Delete { .. } => "delete",
            Self::Rename { .. } => "rename",
            Self::Replace { .. } => "replace",
        }
    }

    /// The `when` guard, if any.
    pub fn when(&self) -> Option<&str> {
        match self {
            Self::Move { when, .. }
            | Self::Reorder { when, .. }
            | Self::Delete { when, .. }
            | Self::Rename { when, .. }
            | Self::Replace { when, .. } => when.as_deref(),
        }
    }

    /// Why the operation cannot run as written.
    fn problem(&self) -> Option<String> {
        let at = |what: &str| format!("{} `{what}`", self.name());
        let cel = |what: &str, source: &str| compile_problem(&at(what), source);
        let template = |what: &str, text: &str| template_problem(&at(what), text);
        if let Some(problem) = self.when().and_then(|when| cel("when", when)) {
            return Some(problem);
        }
        match self {
            Self::Move {
                node,
                before,
                after,
                ..
            } => cel("node", node).or_else(|| match (before, after) {
                (Some(target), None) => cel("before", target),
                (None, Some(target)) => cel("after", target),
                _ => Some("move needs exactly one of `before` or `after`".to_owned()),
            }),
            Self::Reorder { by, .. } => {
                let mut seen = BTreeSet::new();
                if by.is_empty() {
                    return Some("reorder needs at least one key in `by`".to_owned());
                }
                by.iter()
                    .find(|key| !key.contains('/') || !seen.insert(*key))
                    .map(|key| {
                        format!(
                            "reorder key `{key}` must be a qualified `plugin/name` and listed once"
                        )
                    })
            }
            Self::Delete {
                node, file, span, ..
            } => match (node, file, span) {
                (Some(node), None, None) => cel("node", node),
                (None, Some(file), Some(span)) => cel("file", file).or_else(|| cel("span", span)),
                _ => Some("delete needs `node`, or `file` and `span`".to_owned()),
            },
            Self::Rename { symbol, name, .. } => {
                cel("symbol", symbol).or_else(|| template("name", name))
            }
            Self::Replace {
                file, span, text, ..
            } => cel("file", file)
                .or_else(|| cel("span", span))
                .or_else(|| template("text", text)),
        }
    }
}

/// An external program that fixes a finding, by the process contract: exit `0`
/// is success and the result is per `output` (an empty result changes nothing),
/// `1` declines with the reason on stderr, anything else (`>= 2`, a signal, a
/// timeout) is an error that applies nothing. stdout carries only the result;
/// stderr is for people. It runs without a shell, in a scratch directory over
/// the finding's file, with an environment cleared to `PATH`, `HOME`, `LANG`,
/// `TMPDIR`, the declared `env` and `LIGHTHOUSE_DECISION`, `LIGHTHOUSE_OPTIONS`,
/// `LIGHTHOUSE_API_VERSION`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    /// Program and arguments; `{file}`, `{line}`, `{symbol}` and `{rule}` fill
    /// whole arguments.
    pub argv: Vec<String>,
    #[serde(default)]
    pub output: Output,
    #[serde(default)]
    pub stdin: Stdin,
    /// Extra environment variables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// How long it may run, such as `30s` or `2m`.
    #[serde(default = "default_timeout")]
    pub timeout: Duration,
}

impl CommandSpec {
    /// Why the command cannot run as written.
    fn problem(&self) -> Option<String> {
        const KNOWN: [&str; 4] = ["{file}", "{line}", "{symbol}", "{rule}"];
        if let Some(problem) = mixed_placeholder(&self.argv, &KNOWN, |arg| {
            arg.contains("{root}") || KNOWN.iter().any(|p| arg.contains(p))
        }) {
            return Some(problem);
        }
        self.argv
            .first()
            .is_none_or(|program| program.trim().is_empty())
            .then(|| "`argv` needs a program".to_owned())
    }
}

fn default_timeout() -> Duration {
    DEFAULT_TIMEOUT
}
