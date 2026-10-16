//! The `check:` block of a decision: how a violation is detected. One
//! provider kind is selected by `type`; each compiles into the single `Rule`
//! interface. Further kinds join the same union.

use std::{
    collections::{BTreeMap, BTreeSet},
    time,
};

use lighthouse_model::{Capability, EdgeKind, RunScope};
use lighthouse_resource::Duration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    Batch, CommandCheck, ModelCheck, Subject,
    expression::{compile_problem, template_problem},
    union::{refuse_strays, stray_field},
};

/// How long a provider may run when the check names no `timeout`.
pub const DEFAULT_TIMEOUT: time::Duration = time::Duration::from_secs(30);
/// The id of the builtin check that compares a file with its formatter's output.
pub const FORMATTED: &str = "core/formatted";

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
    pub timeout: Option<Duration>,
    #[serde(flatten)]
    pub kind: CheckKind,
}

/// The fields of a check as it is read, before its `type` has had its say
/// about which of them may be there.
#[derive(Deserialize)]
struct Written {
    #[serde(default)]
    requires: Vec<Capability>,
    #[serde(default)]
    timeout: Option<Duration>,
    #[serde(flatten)]
    kind: CheckKind,
}

impl Check {
    /// A check of this kind with no common fields set.
    pub fn new(kind: CheckKind) -> Self {
        Self {
            requires: Vec::new(),
            timeout: None,
            kind,
        }
    }
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
                Some("cycle") => &["op", "edge", "level"],
                Some(other) => {
                    return Err(D::Error::custom(format!(
                        "builtin `op` is `{other}`, expected order or cycle"
                    )));
                }
            },
            Some("cel") => &[
                "select", "with", "where", "message", "evidence", "at", "identity", "review",
                "prompt",
            ],
            Some("command") => &[
                "argv",
                "batch",
                "stdin",
                "env",
                "exitCodes",
                "output",
                "columns",
                "select",
            ],
            Some("model") => &["over", "with", "select", "prompt", "identity"],
            other => {
                return Err(D::Error::custom(match other {
                    None => "a check needs a `type`: builtin, cel, command or model".to_owned(),
                    Some(found) => {
                        format!(
                            "check `type` is `{found}`, expected builtin, cel, command or model"
                        )
                    }
                }));
            }
        };
        if let Some(problem) = stray_field(table, "check", &COMMON, own) {
            return Err(D::Error::custom(problem));
        }
        let Written {
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
            Self::Model(_) => "model",
        }
    }

    /// What is wrong with the provider this names, for a decision about `subject`.
    pub(crate) fn problem(&self, subject: Subject) -> Option<String> {
        match self {
            Self::Builtin(BuiltinCheck::Named(named)) if named.id.is_empty() => {
                Some("a builtin check needs a rule id".to_owned())
            }
            Self::Builtin(BuiltinCheck::Named(_)) => None,
            Self::Builtin(BuiltinCheck::Op(op)) => op.problem(subject),
            Self::Cel(cel) => cel.problem().or_else(|| {
                let Some(select) = cel.selects(subject) else {
                    return Some(format!("a `{subject}` decision needs a `select`"));
                };
                if select.scope() != subject.run_scope() {
                    return Some(format!(
                        "selects `{}`, which a `{subject}` decision cannot run over",
                        select.name()
                    ));
                }
                // A model cannot be asked about a subject outside a file; a
                // module is left to agent review, which lists its subjects.
                (cel.review.is_some()
                    && select.scope() != RunScope::File
                    && select != Select::Module)
                    .then(|| {
                        format!(
                            "a model cannot judge `{}`: it needs the source of a subject in a file",
                            select.name()
                        )
                    })
            }),
            Self::Command(command) => command.problem().or_else(|| {
                (command.batch == Batch::All && subject.run_scope() == RunScope::File).then(|| {
                    "check `batch: all` needs a module or project decision: a file-scope decision is checked one file at a time"
                        .to_owned()
                })
            }),
            Self::Model(model) => model.problem(subject),
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

/// What an `order` check judges, and what a `reorder` fix permutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OrderScope {
    /// Every declaration of a file, each container on its own.
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
    /// No dependency cycle over edges of one kind.
    Cycle {
        /// The edge kind followed: `calls`, `references`, `imports`,
        /// `contains`, `implements` or `accesses-private`.
        edge: String,
        level: CycleLevel,
    },
}

impl BuiltinOp {
    /// What is wrong with the operation for a decision about `subject`: a
    /// standard operation judges the shape of one file or of the whole project.
    fn problem(&self, subject: Subject) -> Option<String> {
        let own = match self {
            Self::Order { clauses } if clauses.is_empty() => {
                Some("check: `order` needs at least one clause".to_owned())
            }
            Self::Order { clauses } => clauses.iter().find_map(OrderClause::problem),
            Self::Cycle { edge, .. } => edge
                .parse::<EdgeKind>()
                .is_err()
                .then(|| format!("check: `{edge}` is not an edge kind")),
        };
        let per_file = subject.run_scope() == RunScope::File;
        own.or_else(|| match self {
            Self::Order { .. } if !per_file => Some(format!(
                "an `order` check judges one file at a time, which a `{subject}` decision does not"
            )),
            Self::Cycle { .. } if per_file => Some(format!(
                "a `cycle` check judges the whole project, which a `{subject}` decision does not"
            )),
            _ => None,
        })
    }
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

impl OrderClause {
    fn problem(&self) -> Option<String> {
        let mut seen = BTreeSet::new();
        if self.by.is_empty() {
            return Some("check: an `order` clause needs at least one key in `by`".to_owned());
        }
        if let Some(key) = self
            .by
            .iter()
            .find(|key| !key.contains('/') || !seen.insert(*key))
        {
            return Some(format!(
                "check: order key `{key}` must be a qualified `plugin/name` and listed once"
            ));
        }
        self.when
            .as_deref()
            .and_then(|source| compile_problem("check when", source))
            .or_else(|| {
                self.evidence.iter().find_map(|(name, source)| {
                    compile_problem(&format!("check evidence `{name}`"), source)
                })
            })
            .or_else(|| template_problem("check message", &self.message))
    }
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
    /// A body event of a function (a panic, an `unwrap`, an error compared
    /// with `==`, ...): one finding per event, at the event. The event has
    /// its `kind`, `detail`, `line` and the function it is in (`event.func`).
    Event,
    /// A decision document of the project: its name, pack, labels and kind,
    /// and where it is written.
    Decision,
}

impl Select {
    const ALL: [Self; 9] = [
        Self::Symbol,
        Self::Function,
        Self::Edge,
        Self::Module,
        Self::File,
        Self::Test,
        Self::Comment,
        Self::Event,
        Self::Decision,
    ];

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
            Self::Event => "event",
            Self::Decision => "decision",
        }
    }
}

/// A rule written as data: a `where` expression that is true for a violation
/// of the selected values, and a message. The expressions see the selected
/// value (`symbol`, `func`, `edge`, `module`, `file`, `test`, `comment` or
/// `event`),
/// the decision's resolved `options`, and the standard function library over
/// the code model: `metrics(n)`, `callers(n)`, `callees(n)`, `edges(n, kind)`,
/// `owner(n)`, `tests(n)`, `annotations(n)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    /// CEL over the selected value, evaluated when `where` is false: when it
    /// is true, or gives a kind (a string), a model is asked and the
    /// subject is a finding, with this check's message, evidence and
    /// identity, only if the model says `fail`. An unbound or unsure model
    /// leaves the subject to agent review. A check that cannot be proven by
    /// the code model but can be suspected by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    /// What the model is told about `review`'s subjects. Default: the
    /// decision's requirement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
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
        for binding in &self.bindings {
            let name = binding.name.as_str();
            let valid = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
                && name != "options"
                && !Select::ALL.iter().any(|s| s.variable() == name);
            if !valid {
                return Some(format!("check with: `{name}` is not a usable name"));
            }
            if let Some(problem) = compile_problem(&format!("check with `{name}`"), &binding.expr) {
                return Some(problem);
            }
        }
        if let Some(problem) = compile_problem("check where", &self.condition) {
            return Some(problem);
        }
        if let Some(problem) = self.review_problem() {
            return Some(problem);
        }
        for (name, source) in &self.evidence {
            if let Some(problem) = compile_problem(&format!("check evidence `{name}`"), source) {
                return Some(problem);
            }
        }
        let positions = self.at.iter().flat_map(|at| {
            [
                ("at.symbol", at.symbol.as_ref()),
                ("at.line", at.line.as_ref()),
                ("at.endLine", at.end_line.as_ref()),
            ]
        });
        let identity = self.identity.iter().flat_map(|identity| {
            [
                ("identity.subject", Some(&identity.subject)),
                ("identity.snippet", identity.snippet.as_ref()),
            ]
        });
        for (what, source) in positions.chain(identity) {
            if let Some(problem) =
                source.and_then(|source| compile_problem(&format!("check {what}"), source))
            {
                return Some(problem);
            }
        }
        template_problem("check message", &self.message)
    }

    /// What is wrong with `review` or `prompt`.
    fn review_problem(&self) -> Option<String> {
        if self.review.is_none() && self.prompt.is_some() {
            return Some("check: `prompt` needs a `review`".to_owned());
        }
        if self.prompt.as_deref().is_some_and(|p| p.trim().is_empty()) {
            return Some("check: `prompt` is empty".to_owned());
        }
        compile_problem("check review", self.review.as_deref()?)
    }
}
