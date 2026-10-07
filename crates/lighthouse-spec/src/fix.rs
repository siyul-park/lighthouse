//! The `fix:` block of a pattern: how the finding of its rule is fixed. A
//! fix is data, never code of the rule: its kind names how the proposal is
//! made (generic operations over the code model, an external command) and
//! every kind compiles into a provider of the one `Fixer` interface.

use std::collections::{BTreeMap, BTreeSet};

use cel::Program;
use lighthouse_model::{Capability, Safety};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Error;

/// How a pattern's findings are fixed and how far the fix may be trusted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawFix", into = "RawFix")]
pub struct Fix {
    /// Caps what the fixer may claim: a `safe` proposal under a `suggested`
    /// pattern is downgraded. `safe` is reserved for mechanical patterns.
    pub safety: Safety,
    /// Provider capabilities the fix needs; without them it is declined.
    pub requires: Vec<Capability>,
    pub kind: FixKind,
}

/// The kind of a fix; a `fix:` block holds exactly one.
#[derive(Debug, Clone, PartialEq)]
pub enum FixKind {
    /// Generic operations over the code model, evaluated with CEL.
    Ops(Vec<OpSpec>),
    /// An external program on a scratch copy of the files.
    Command(CommandSpec),
    /// A language plugin's fix method. Reserved: not yet supported.
    Rpc(Value),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFix {
    safety: Safety,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requires: Vec<Capability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ops: Option<Vec<OpSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<CommandSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rpc: Option<Value>,
}

impl TryFrom<RawFix> for Fix {
    type Error = String;

    fn try_from(raw: RawFix) -> Result<Self, String> {
        let kind = match (raw.ops, raw.command, raw.rpc) {
            (Some(ops), None, None) => FixKind::Ops(ops),
            (None, Some(command), None) => FixKind::Command(command),
            (None, None, Some(rpc)) => FixKind::Rpc(rpc),
            _ => return Err("a fix needs exactly one of `ops`, `command` or `rpc`".to_owned()),
        };
        Ok(Self {
            safety: raw.safety,
            requires: raw.requires,
            kind,
        })
    }
}

impl From<Fix> for RawFix {
    fn from(fix: Fix) -> Self {
        let (mut ops, mut command, mut rpc) = (None, None, None);
        match fix.kind {
            FixKind::Ops(o) => ops = Some(o),
            FixKind::Command(c) => command = Some(c),
            FixKind::Rpc(r) => rpc = Some(r),
        }
        Self {
            safety: fix.safety,
            requires: fix.requires,
            ops,
            command,
            rpc,
        }
    }
}

/// The declarations a `reorder` permutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandOutput {
    /// The command edits the scratch copy of the file.
    #[default]
    InPlace,
    /// The command prints the new text of the file on stdout, and nothing else.
    Text,
}

/// What the command is given on stdin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandStdin {
    #[default]
    None,
    /// The content of the target file.
    File,
}

/// Where a command's edits may land.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Seconds in a timeout written as `30s`, `2m` or a bare number of seconds.
pub fn timeout_seconds(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let n: u64 = text[..split].parse().ok()?;
    match &text[split..] {
        "" | "s" => Some(n),
        "m" => n.checked_mul(60),
        _ => None,
    }
}

/// The checks of a catalog on a pattern's `fix:`.
pub(crate) fn validate(
    id: &str,
    fix: &Fix,
    mechanical: bool,
    implemented: bool,
) -> Result<(), Error> {
    let fail = |reason: String| Error::invalid(id, format!("fix: {reason}"));
    if !implemented {
        return Err(fail(
            "a pattern without an implementation has no findings to fix".to_owned(),
        ));
    }
    if fix.safety == Safety::Safe && !mechanical {
        return Err(fail(
            "`safe` is reserved for mechanical patterns; use `suggested`".to_owned(),
        ));
    }
    match &fix.kind {
        FixKind::Ops(ops) => {
            if ops.is_empty() {
                return Err(fail("`ops` is empty".to_owned()));
            }
            let renames = ops.iter().any(|op| matches!(op, OpSpec::Rename { .. }));
            if renames && !fix.requires.contains(&Capability::CompleteReferences) {
                return Err(fail(
                    "`rename` needs `complete-references` in `requires`".to_owned(),
                ));
            }
            ops.iter().try_for_each(|op| op_valid(op).map_err(&fail))
        }
        FixKind::Command(command) => command_valid(command).map_err(fail),
        // Kept, and declined when a fix is asked for; see `FixPlan`.
        FixKind::Rpc(_) => Ok(()),
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
        return Err("`command.argv` needs a program".to_owned());
    }
    if timeout_seconds(&command.timeout).is_none_or(|s| s == 0) {
        return Err(format!(
            "`command.timeout` is `{}`, expected a number of seconds or minutes such as `30s`",
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
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open..];
        let close = after
            .find("}}")
            .ok_or_else(|| format!("{what}: `{{{{` is never closed"))?;
        cel(what, after[2..close].trim())?;
        rest = &after[close + 2..];
    }
    Ok(())
}
