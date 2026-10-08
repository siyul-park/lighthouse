//! The `fix:` block of a decision: how the finding of its rule is fixed. A
//! fix is data, never code of the rule: its `type` names how the proposal is
//! made (generic operations over the code model, an external command) and
//! every type compiles into a provider of the one `Fixer` interface.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use cel::Program;
use lighthouse_model::{Capability, Safety};
use lighthouse_resource::parse_duration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Error;

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
struct Shape {
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
            Some("command") => &["argv", "output", "stdin", "env", "timeout", "scope"],
            Some("rpc") => &["params"],
            other => {
                return Err(D::Error::custom(match other {
                    None => "a fix needs a `type`: ops, command or rpc".to_owned(),
                    Some(found) => format!("fix `type` is `{found}`, expected ops, command or rpc"),
                }));
            }
        };
        if let Some(stray) = table
            .keys()
            .find(|key| !COMMON.contains(&key.as_str()) && !own.contains(&key.as_str()))
        {
            return Err(D::Error::custom(format!(
                "unknown field `{stray}` for a fix of type `{}`",
                table["type"].as_str().unwrap_or_default()
            )));
        }
        let Shape {
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

/// The type of a fix; a `fix:` block holds exactly one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FixKind {
    /// Generic operations over the code model, evaluated with CEL.
    Ops { ops: Vec<OpSpec> },
    /// An external program on a scratch copy of the files.
    Command(CommandSpec),
    /// A language plugin's fix method. Reserved: not yet supported.
    Rpc {
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        params: Map<String, Value>,
    },
}

/// The declarations a `reorder` permutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ReorderScope {
    /// Every declaration of the finding's file, each container sorted on its own.
    File,
    /// Only the members of the finding symbol's owner.
    Owner,
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
        scope: ReorderScope,
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
}

/// What the orchestrator takes from a command that succeeded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CommandOutput {
    /// The command edits the scratch copy of the file.
    #[default]
    InPlace,
    /// The command prints the new text of the file on stdout, and nothing else.
    Text,
}

/// What the command is given on stdin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CommandStdin {
    #[default]
    None,
    /// The content of the target file.
    File,
}

/// Where a command's edits may land.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CommandScope {
    /// Only the finding's file.
    #[default]
    File,
}

/// An external program that fixes a finding, by the process contract: exit `0`
/// is success and the result is per `output` (an empty result changes nothing),
/// `1` declines with the reason on stderr, anything else (`>= 2`, a signal, a
/// timeout) is an error that applies nothing. stdout carries only the result;
/// stderr is for people. It runs without a shell, in a scratch directory, with
/// an environment cleared to `PATH`, `HOME`, `LANG`, `TMPDIR`, the declared
/// `env` and `LIGHTHOUSE_DECISION`, `LIGHTHOUSE_OPTIONS`, `LIGHTHOUSE_API_VERSION`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    /// Program and arguments; `{file}`, `{line}`, `{symbol}` and `{rule}` fill
    /// whole arguments.
    pub argv: Vec<String>,
    #[serde(default)]
    pub output: CommandOutput,
    #[serde(default)]
    pub stdin: CommandStdin,
    /// Extra environment variables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// How long it may run, such as `30s` or `2m`.
    #[serde(default = "default_timeout")]
    pub timeout: String,
    #[serde(default)]
    pub scope: CommandScope,
}

impl CommandSpec {
    /// How long the command may run; `None` for a text that is not a
    /// duration such as `30s`.
    pub fn timeout_duration(&self) -> Option<Duration> {
        parse_duration(&self.timeout)
    }
}

/// The checks of a catalog on a decision's `fix:`.
pub(crate) fn validate(id: &str, fix: &Fix, mechanical: bool, checked: bool) -> Result<(), Error> {
    let fail = |reason: String| Error::invalid(id, format!("fix: {reason}"));
    if !checked {
        return Err(fail(
            "a decision without a check has no findings to fix".to_owned(),
        ));
    }
    if fix.safety == Safety::Safe && !mechanical {
        return Err(fail(
            "`safe` is reserved for decisions that author `error`; use `suggested`".to_owned(),
        ));
    }
    match &fix.kind {
        FixKind::Ops { ops } => {
            if ops.is_empty() {
                return Err(fail("`ops` is empty".to_owned()));
            }
            let renames = ops.iter().any(|op| matches!(op, OpSpec::Rename { .. }));
            if renames && !fix.requires.contains(&Capability::CompleteReferences) {
                return Err(fail(
                    "`rename` needs `complete-references` in `requires`".to_owned(),
                ));
            }
            ops.iter().try_for_each(|op| op_valid(op).map_err(fail))
        }
        FixKind::Command(command) => command_valid(command).map_err(fail),
        // Kept, and declined when a fix is asked for; see `FixPlan`.
        FixKind::Rpc { .. } => Ok(()),
    }
}

fn default_timeout() -> String {
    "30s".to_owned()
}

fn command_valid(command: &CommandSpec) -> Result<(), String> {
    if let Some(arg) = command.argv.iter().find(|a| bad_placeholder(a)) {
        return Err(format!(
            "`{arg}` is not a placeholder of its own: `{{file}}`, `{{line}}`, `{{symbol}}` and `{{rule}}` fill a whole argument"
        ));
    }
    if command
        .argv
        .first()
        .is_none_or(|program| program.trim().is_empty())
    {
        return Err("`argv` needs a program".to_owned());
    }
    if command.timeout_duration().is_none_or(|t| t.is_zero()) {
        return Err(format!(
            "`timeout` is `{}`, expected a duration such as `30s` or `2m`",
            command.timeout
        ));
    }
    Ok(())
}

/// An argument that uses a placeholder without being exactly that placeholder
/// (`--out={file}`), or names one that does not exist (`{root}`).
fn bad_placeholder(arg: &str) -> bool {
    const KNOWN: [&str; 4] = ["{file}", "{line}", "{symbol}", "{rule}"];
    if KNOWN.contains(&arg) {
        return false;
    }
    KNOWN.iter().any(|p| arg.contains(p)) || arg.contains("{root}")
}

fn op_valid(op: &OpSpec) -> Result<(), String> {
    let at = |what: &str| format!("{} `{what}`", op.name());
    if let Some(when) = op.when() {
        cel(&at("when"), when)?;
    }
    match op {
        OpSpec::Move {
            node,
            before,
            after,
            ..
        } => move_valid(&at, node, before.as_deref(), after.as_deref()),
        OpSpec::Reorder { by, .. } => reorder_valid(by),
        OpSpec::Delete {
            node, file, span, ..
        } => delete_valid(&at, node.as_deref(), file.as_deref(), span.as_deref()),
        OpSpec::Rename { symbol, name, .. } => {
            cel(&at("symbol"), symbol)?;
            template(&at("name"), name)
        }
        OpSpec::Replace {
            file, span, text, ..
        } => {
            cel(&at("file"), file)?;
            cel(&at("span"), span)?;
            template(&at("text"), text)
        }
    }
}

fn move_valid(
    at: &dyn Fn(&str) -> String,
    node: &str,
    before: Option<&str>,
    after: Option<&str>,
) -> Result<(), String> {
    cel(&at("node"), node)?;
    match (before, after) {
        (Some(target), None) => cel(&at("before"), target),
        (None, Some(target)) => cel(&at("after"), target),
        _ => Err("move needs exactly one of `before` or `after`".to_owned()),
    }
}

fn reorder_valid(by: &[String]) -> Result<(), String> {
    if by.is_empty() {
        return Err("reorder needs at least one key in `by`".to_owned());
    }
    let mut seen = BTreeSet::new();
    for key in by {
        if !key.contains('/') || !seen.insert(key) {
            return Err(format!(
                "reorder key `{key}` must be a qualified `plugin/name` and listed once"
            ));
        }
    }
    Ok(())
}

fn delete_valid(
    at: &dyn Fn(&str) -> String,
    node: Option<&str>,
    file: Option<&str>,
    span: Option<&str>,
) -> Result<(), String> {
    match (node, file, span) {
        (Some(node), None, None) => cel(&at("node"), node),
        (None, Some(file), Some(span)) => {
            cel(&at("file"), file)?;
            cel(&at("span"), span)
        }
        _ => Err("delete needs `node`, or `file` and `span`".to_owned()),
    }
}

fn cel(what: &str, source: &str) -> Result<(), String> {
    Program::compile(source)
        .map(drop)
        .map_err(|e| format!("{what}: {e}"))
}

/// Every `{{ cel }}` hole of a template compiles and is closed.
fn template(what: &str, text: &str) -> Result<(), String> {
    for part in crate::check::parse_template(text).map_err(|e| format!("{what}: {e}"))? {
        if let crate::check::TemplatePart::Hole(source) = part {
            cel(what, &source)?;
        }
    }
    Ok(())
}

/// The loader refuses a field its `type` does not have; so does the schema:
/// each branch carries the common fields too and allows no other.
fn refuse_strays(schema: &mut schemars::Schema) {
    let Some(root) = schema.as_object_mut() else {
        return;
    };
    let common = match root.get("properties") {
        Some(Value::Object(properties)) => properties.clone(),
        _ => Map::new(),
    };
    let required = root
        .get("required")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let Some(Value::Array(branches)) = root.get_mut("oneOf") else {
        return;
    };
    for branch in branches.iter_mut().filter_map(Value::as_object_mut) {
        if let Value::Object(properties) = branch
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()))
        {
            properties.extend(common.clone());
        }
        if let (Some(Value::Array(own)), Value::Array(shared)) =
            (branch.get_mut("required"), &required)
        {
            own.extend(shared.iter().cloned());
        }
        branch.insert("additionalProperties".to_owned(), Value::Bool(false));
    }
}
