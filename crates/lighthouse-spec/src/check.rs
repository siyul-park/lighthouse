//! The `check:` block of a decision: how a violation is detected. One
//! provider kind is selected by `type`; each compiles into the single `Rule`
//! interface. Further kinds (`command`, `rpc`, ...) join the same union.

use std::{collections::BTreeMap, time::Duration};

use cel::Program;
use lighthouse_model::{Capability, RunScope};

use crate::Subject;
use lighthouse_resource::parse_duration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// How long a provider may run when the check names no `timeout`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How a decision is checked, mirroring [`crate::Fix`]: one provider selected by
/// `type`, with the fields every provider shares. Which other fields a check
/// may have depends on its `type`; any other is refused.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[schemars(transform = refuse_strays)]
pub struct Check {
    /// Capabilities the language provider must offer; without them the check
    /// is not run for that language.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<Capability>,
    /// How long one run of the provider may take, such as `30s` or `2m`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    #[serde(flatten)]
    pub kind: CheckKind,
}

/// The fields of a check as it is read, before its `type` has had its say
/// about which of them may be there.
#[derive(Deserialize)]
struct Shape {
    #[serde(default)]
    requires: Vec<Capability>,
    #[serde(default)]
    timeout: Option<String>,
    #[serde(flatten)]
    kind: CheckKind,
}

impl<'de> Deserialize<'de> for Check {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        const COMMON: [&str; 3] = ["requires", "timeout", "type"];
        let value = Value::deserialize(deserializer)?;
        let table = value
            .as_object()
            .ok_or_else(|| D::Error::custom("a check is a table"))?;
        let own: &[&str] = match table.get("type").and_then(Value::as_str) {
            Some("builtin") => match table.get("op").and_then(Value::as_str) {
                None if table.contains_key("op") => {
                    return Err(D::Error::custom("a builtin `op` is a name"));
                }
                None => &["id"],
                Some("order") => &["op", "clauses"],
                Some("proximity") => &[
                    "op",
                    "when",
                    "group",
                    "separator",
                    "maxDistance",
                    "contiguous",
                    "message",
                    "evidence",
                ],
                Some("cycle") => &["op", "edge", "level"],
                Some(other) => {
                    return Err(D::Error::custom(format!(
                        "builtin `op` is `{other}`, expected order, proximity or cycle"
                    )));
                }
            },
            Some("cel") => &[
                "select", "with", "where", "message", "evidence", "at", "identity",
            ],
            Some("command") => &["argv", "batch", "stdin", "env", "exitCodes"],
            Some("rpc") => &["params"],
            Some("model") => &["select", "prompt"],
            other => {
                return Err(D::Error::custom(match other {
                    None => {
                        "a check needs a `type`: builtin, cel, command, rpc or model".to_owned()
                    }
                    Some(found) => {
                        format!(
                            "check `type` is `{found}`, expected builtin, cel, command, rpc or model"
                        )
                    }
                }));
            }
        };
        if let Some(stray) = table
            .keys()
            .find(|key| !COMMON.contains(&key.as_str()) && !own.contains(&key.as_str()))
        {
            return Err(D::Error::custom(format!(
                "unknown field `{stray}` for a check of type `{}`",
                table["type"].as_str().unwrap_or_default()
            )));
        }
        let Shape {
            requires,
            timeout,
            kind,
        } = serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(Self {
            requires,
            timeout,
            kind,
        })
    }
}

impl Check {
    /// A check of this kind with no common fields set.
    pub fn of(kind: CheckKind) -> Self {
        Self {
            requires: Vec::new(),
            timeout: None,
            kind,
        }
    }

    /// How long a run may take: the `timeout`, else the default.
    /// Whether a program decides the findings of this check, as opposed to
    /// agent review.
    pub fn is_automated(&self) -> bool {
        self.kind.deterministic()
    }

    /// How long a run may take: the `timeout`, else the default; `None` for a
    /// text that is not a duration.
    pub fn timeout_duration(&self) -> Option<Duration> {
        match &self.timeout {
            None => Some(DEFAULT_TIMEOUT),
            Some(text) => parse_duration(text),
        }
    }
}

impl CheckKind {
    /// Whether the same input always gives the same findings. Only a model is
    /// not; an execution error of a deterministic check leaves the analysis
    /// incomplete, and that of any other is skipped with a notice.
    pub fn deterministic(&self) -> bool {
        !matches!(self, Self::Model(_))
    }

    /// The name of what runs, for documents: the operation of a standard
    /// builtin, else the `type`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Builtin(BuiltinCheck::Op(op)) => match op {
                BuiltinOp::Order { .. } => "order",
                BuiltinOp::Proximity { .. } => "proximity",
                BuiltinOp::Cycle { .. } => "cycle",
            },
            other => other.name(),
        }
    }

    /// The `type` as written in a decision file.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Builtin(_) => "builtin",
            Self::Cel(_) => "cel",
            Self::Command(_) => "command",
            Self::Rpc(_) => "rpc",
            Self::Model(_) => "model",
        }
    }
}

/// The type of a check; a `check:` block holds exactly one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum CheckKind {
    /// A standard operation, or a rule registered by a bundled plugin.
    Builtin(BuiltinCheck),
    /// A CEL expression over the code model.
    Cel(CelCheck),
    /// An external program under the process contract.
    Command(CommandCheck),
    /// A language plugin's check method. Reserved until protocol 0.2.
    Rpc(RpcCheck),
    /// Agent review: a model is asked about the candidates. Not
    /// deterministic; today's agent review tasks serve it.
    Model(ModelCheck),
}

/// A builtin check: one of the standard, decision-agnostic operations, or a
/// rule registered by name that no standard operation can express.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum BuiltinCheck {
    Op(BuiltinOp),
    Named(NamedRule),
}

/// A rule implemented in Rust and registered by a bundled plugin. Strays are
/// refused by [`Check`], which knows the fields every provider shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NamedRule {
    /// The id of the registered rule.
    pub id: String,
}

impl BuiltinCheck {
    /// The id of the registered rule, for a named one.
    pub fn named(&self) -> Option<&str> {
        match self {
            Self::Named(rule) => Some(&rule.id),
            Self::Op(_) => None,
        }
    }
}

/// What an `order` check judges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OrderScope {
    /// Every declaration of a file, each module on its own.
    File,
    /// Only the members of one owner.
    Owner,
}

/// A standard operation over the code model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum BuiltinOp {
    /// Declarations must follow the order of registered order keys, the same
    /// keys a `reorder` fix sorts by. Each clause is judged on its own and the
    /// findings are concatenated.
    Order { clauses: Vec<OrderClause> },
    /// Declarations that belong together stay together: nothing but members
    /// of the same `group` may separate two of them.
    Proximity {
        /// CEL over `file`: the check runs on the files it is true for.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<String>,
        /// CEL over a declaration (`symbol`) that gives the key of the group
        /// it belongs to; empty when it belongs to none.
        group: String,
        /// CEL over `between`, a declaration lying between two members of the
        /// group, and `member` (the later one) and `key` (the group's key):
        /// true for a declaration that splits the group. Default: any
        /// declaration that is not in the group.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        separator: Option<String>,
        /// How many separating declarations a group tolerates between two of
        /// its members.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_distance: Option<u32>,
        /// The group is contiguous: no separator at all. The default when
        /// `maxDistance` is not given.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        contiguous: bool,
        /// Text over `symbol` (the member separated from the group),
        /// `previous` (the member before it), `separators` (the declarations
        /// between, as a list), `count` and `options`.
        message: String,
        /// Evidence fields, each a CEL expression over the same variables.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        evidence: BTreeMap<String, String>,
    },
    /// No dependency cycle over edges of one kind.
    Cycle {
        /// The edge kind followed: `calls`, `references`, `imports`,
        /// `contains`, `implements` or `accesses-private`.
        edge: String,
        level: CycleLevel,
    },
}

/// How an `order` clause picks the declarations to report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OrderReport {
    /// The fewest declarations that, moved, would put the container in order:
    /// those outside its longest in-order run.
    #[default]
    Displaced,
    /// Every declaration that has an earlier one of a higher rank.
    Late,
}

/// One ordering a container of declarations must follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OrderClause {
    /// What is ordered: each module of a file, or each owner's members.
    pub within: OrderScope,
    /// Qualified `plugin/name` order keys, most significant first.
    pub by: Vec<String>,
    #[serde(default)]
    pub report: OrderReport,
    /// CEL over `file` and `options`: the clause runs on the files it is
    /// true for. Default: every file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// Text with `{{ cel }}` holes over `symbol` (the declaration reported),
    /// `rank` and `ranks` (its rank by the first key and by every key),
    /// `after` (with `displaced`: the declaration it must follow, else an
    /// empty node), `earlier` (with `late`: the first declaration before it
    /// that ranks higher) and `options`.
    pub message: String,
    /// Evidence fields, each a CEL expression over the same variables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub evidence: BTreeMap<String, String>,
}

/// At what level a `cycle` check looks for strongly connected components.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CycleLevel {
    Module,
    Symbol,
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
    /// A comment of a file, with its text and position.
    Comment,
    /// A decision document of the project: its name, pack, labels and kind,
    /// and where it is written.
    Decision,
}

impl Select {
    /// What a check looks at when it does not say: the subject of the
    /// decision. A `project` decision has no such default.
    pub fn of(subject: Subject) -> Option<Self> {
        match subject {
            Subject::Symbol => Some(Self::Symbol),
            Subject::File => Some(Self::File),
            Subject::Module => Some(Self::Module),
            Subject::Edge => Some(Self::Edge),
            Subject::Test => Some(Self::Test),
            Subject::Decision => Some(Self::Decision),
            Subject::Project => None,
        }
    }

    /// Edges and modules are judged once over the project; the rest per file.
    pub fn scope(self) -> RunScope {
        match self {
            Self::Edge | Self::Module | Self::Decision => RunScope::Project,
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
            Self::Comment => "comment",
            Self::Decision => "decision",
        }
    }
}

/// A rule written as data: a `where` expression that is true for a violation
/// of the selected values, and a message. The expressions see the selected
/// value (`symbol`, `func`, `edge`, `module`, `file`, `test` or `comment`),
/// the decision's resolved `options`, and the standard function library over
/// the code model: `metrics(n)`, `callers(n)`, `callees(n)`, `edges(n, kind)`,
/// `owner(n)`, `tests(n)`, `annotations(n)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CelCheck {
    /// What the check looks at. Default: what the decision's `scope.subject`
    /// is about; a decision about the project says which it looks at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<Select>,
    /// Named intermediate values, evaluated in order before `where`; each
    /// expression sees the ones before it.
    #[serde(default, rename = "with", skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<Binding>,
    /// CEL; true for a violation.
    #[serde(rename = "where")]
    pub condition: String,
    /// Text with `{{ cel }}` holes.
    pub message: String,
    /// Evidence fields, each a CEL expression.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub evidence: BTreeMap<String, String>,
    /// Where the finding is reported when it is not at the selected value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<At>,
    /// What makes a finding the same finding from one run to the next, when
    /// the default (the selected value) does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
}

/// A named CEL value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// A lower-case identifier that no expression variable already uses.
    pub name: String,
    /// CEL.
    pub expr: String,
}

/// Where a finding is reported. Every field is a CEL expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct At {
    /// The id of a symbol: the finding is reported at that symbol and cites it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// The first line, for a finding about a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// The last line, for a finding about a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<String>,
}

/// The identity of a finding: its fingerprint is a hash of the decision, the
/// `subject` and the whitespace-normalized `snippet`. Both are CEL expressions
/// that give text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

impl CelCheck {
    /// What the check looks at for a decision about `subject`: its `select`,
    /// else the subject's own; `None` for a project decision that names none.
    pub fn selects(&self, subject: Subject) -> Option<Select> {
        self.select.or_else(|| Select::of(subject))
    }

    /// The first expression that does not compile, described.
    pub(crate) fn problem(&self) -> Option<String> {
        let compile = |what: &str, source: &str| {
            Program::compile(source)
                .err()
                .map(|e| format!("check {what}: {e}"))
        };
        for binding in &self.bindings {
            let reserved = [
                "options", "symbol", "func", "edge", "module", "file", "test", "comment",
                "decision",
            ];
            let valid = !binding.name.is_empty()
                && binding
                    .name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
                && !reserved.contains(&binding.name.as_str());
            if !valid {
                return Some(format!(
                    "check with: `{}` is not a usable name",
                    binding.name
                ));
            }
            if let Some(problem) = compile(&format!("with `{}`", binding.name), &binding.expr) {
                return Some(problem);
            }
        }
        if let Some(problem) = compile("where", &self.condition) {
            return Some(problem);
        }
        for (name, source) in &self.evidence {
            if let Some(problem) = compile(&format!("evidence `{name}`"), source) {
                return Some(problem);
            }
        }
        let positions = self.at.iter().flat_map(|at| {
            [
                ("at.symbol", &at.symbol),
                ("at.line", &at.line),
                ("at.endLine", &at.end_line),
            ]
        });
        let identity = self.identity.iter().flat_map(|identity| {
            [
                ("identity.subject", Some(&identity.subject)),
                ("identity.snippet", identity.snippet.as_ref()),
            ]
        });
        for (what, source) in positions
            .map(|(what, source)| (what, source.as_ref()))
            .chain(identity)
        {
            if let Some(problem) = source.and_then(|source| compile(what, source)) {
                return Some(problem);
            }
        }
        template_problem(&self.message).map(|p| format!("check message: {p}"))
    }
}

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

/// A check a model answers: the subjects `select` picks are put to it with
/// the decision's examples as its few shots. Read and validated only; agent
/// review tasks serve the decision today.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelCheck {
    /// CEL over the code model that picks the subjects worth asking about.
    /// Default: the subjects that match the decision's scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<String>,
    /// What the model is told. Default: the decision's requirement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

impl ModelCheck {
    pub(crate) fn problem(&self) -> Option<String> {
        if self.prompt.as_deref().is_some_and(|p| p.trim().is_empty()) {
            return Some("check: `prompt` is empty".to_owned());
        }
        Program::compile(self.select.as_deref()?)
            .err()
            .map(|e| format!("check select: {e}"))
    }
}

/// A language plugin's check method. Reserved until protocol 0.2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RpcCheck {
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
}

/// One part of a `{{ cel }}` template: literal text, or the source of a hole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplatePart {
    Text(String),
    Hole(String),
}

/// Splits a template into its text and its `{{ cel }}` holes; the one parser
/// every template of a decision (check messages, fix names and texts) goes
/// through. It does not compile the holes.
pub fn parse_template(text: &str) -> Result<Vec<TemplatePart>, String> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let (before, after) = rest.split_at(open);
        if !before.is_empty() {
            parts.push(TemplatePart::Text(before.to_owned()));
        }
        let Some(close) = after.find("}}") else {
            return Err("`{{` is never closed".to_owned());
        };
        parts.push(TemplatePart::Hole(after[2..close].trim().to_owned()));
        rest = &after[close + 2..];
    }
    if !rest.is_empty() {
        parts.push(TemplatePart::Text(rest.to_owned()));
    }
    Ok(parts)
}

/// Why a `{{ cel }}` template is not well formed.
pub(crate) fn template_problem(text: &str) -> Option<String> {
    let parts = match parse_template(text) {
        Ok(parts) => parts,
        Err(problem) => return Some(problem),
    };
    parts.iter().find_map(|part| match part {
        TemplatePart::Hole(source) => Program::compile(source)
            .err()
            .map(|e| format!("`{source}`: {e}")),
        TemplatePart::Text(_) => None,
    })
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
        // A branch that refers to another schema for its own fields (the
        // builtin operations) cannot close itself with `additionalProperties`,
        // which only sees the properties written beside it.
        let closing = if branch.contains_key("$ref") {
            "unevaluatedProperties"
        } else {
            "additionalProperties"
        };
        branch.insert(closing.to_owned(), Value::Bool(false));
    }
}
