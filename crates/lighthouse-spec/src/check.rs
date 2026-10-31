//! The `check:` block of a decision: how a violation is detected. One
//! provider kind is selected by `type`; each compiles into the single `Rule`
//! interface. Further kinds join the same union.

use std::collections::BTreeMap;

use lighthouse_model::{RunScope, ucm::Capability};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CommandCheck, Env, ModelCheck, Subject, command,
    expression::{compile_problem, template_problem},
    union::{refuse_strays, stray_field},
};

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
    #[serde(flatten)]
    pub kind: CheckKind,
}

/// The fields of a check as it is read, before its `type` has had its say
/// about which of them may be there.
#[derive(Deserialize)]
struct Written {
    #[serde(default)]
    requires: Vec<Capability>,
    #[serde(flatten)]
    kind: CheckKind,
}

impl Check {
    /// A check of this kind with no common fields set.
    pub fn new(kind: CheckKind) -> Self {
        Self {
            requires: Vec::new(),
            kind,
        }
    }
}

impl<'de> Deserialize<'de> for Check {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        const COMMON: [&str; 2] = ["requires", "type"];
        let value = Value::deserialize(deserializer)?;
        let table = value
            .as_object()
            .ok_or_else(|| D::Error::custom("a check is a table"))?;
        let command = command::fields();
        let own: &[&str] = match table.get("type").and_then(Value::as_str) {
            Some("cel") => &[
                "select",
                "variables",
                "where",
                "message",
                "evidence",
                "at",
                "identity",
                "abstain",
                "prompt",
            ],
            Some("command") => &command,
            Some("model") => &["over", "variables", "select", "prompt", "identity"],
            other => {
                return Err(D::Error::custom(match other {
                    None => "a check needs a `type`: cel, command or model".to_owned(),
                    Some(found) => {
                        format!("check `type` is `{found}`, expected cel, command or model")
                    }
                }));
            }
        };
        if let Some(problem) = stray_field(table, "check", &COMMON, own) {
            return Err(D::Error::custom(problem));
        }
        let Written { requires, kind } = serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(Self { requires, kind })
    }
}

impl CheckKind {
    /// Whether the same input always gives the same findings. Only a model is
    /// not; an execution error of a deterministic check leaves the analysis
    /// incomplete, and that of any other is skipped with a notice.
    pub fn deterministic(&self) -> bool {
        !matches!(self, Self::Model(_))
    }

    /// The `type` as written in a decision file.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Cel(_) => "cel",
            Self::Command(_) => "command",
            Self::Model(_) => "model",
        }
    }

    /// What is wrong with the provider this names, for a decision about `subject`.
    pub(crate) fn problem(&self, cel: &Env, subject: Subject) -> Option<String> {
        match self {
            Self::Cel(check) => check.problem(cel).or_else(|| {
                let Some(select) = check.selects(subject) else {
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
                (check.abstain.is_some()
                    && select.scope() != RunScope::File
                    && select != Select::Module)
                    .then(|| {
                        format!(
                            "a model cannot judge `{}`: it needs the source of a subject in a file",
                            select.name()
                        )
                    })
            }),
            Self::Command(command) => command.problem(),
            Self::Model(model) => model.problem(cel, subject),
        }
    }
}

/// The type of a check; a `check:` block holds exactly one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum CheckKind {
    /// A CEL expression over the code model.
    Cel(CelCheck),
    /// An external program under the process contract.
    Command(CommandCheck),
    /// Agent review: a model is asked about the candidates. Not
    /// deterministic; today's agent review tasks serve it.
    Model(ModelCheck),
}

/// A rule written as data: a `where` expression that is true for a violation
/// of the selected values, and a message. The expressions see the selected
/// value (`symbol`, `func`, `edge`, `module`, `file`, `test`, `comment`, `annotation` or
/// `event`),
/// the decision's resolved `options`, and the standard function library over
/// the code model: `metrics(n)`, `callers(n)`, `owner(n)`, `tests(n)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CelCheck {
    /// What the check looks at. Default: what the decision's `scope.subject`
    /// is about; a decision about the project says which it looks at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<Select>,
    /// Named intermediate values, evaluated in order before `where`; each
    /// expression sees the ones before it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<Variable>,
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
    /// The condition under which the check is unsure and leaves the subject to
    /// the next layer. CEL over the selected value, evaluated when `where` is
    /// false: when it is true, or gives a kind (a string), a model is asked and
    /// the subject is a finding, with this check's message, evidence and
    /// identity, only if the model says `fail`. An unbound or unsure model
    /// leaves the subject to agent review. A check that cannot be proven by
    /// the code model but can be suspected by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abstain: Option<String>,
    /// What the model is told about `abstain`'s subjects. Default: the
    /// decision's requirement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
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
    /// A suppression directive of a comment (`lighthouse-disable ...`), one
    /// per rule it names: its `rule`, `reason`, whether it suppressed a
    /// finding (`used`) and the text and place of the directive. Judged after
    /// the other rules, so that what a directive suppressed is known.
    Annotation,
    /// A body event of a function (a panic, an `unwrap`, an error compared
    /// with `==`, ...): one finding per event, at the event. The event has
    /// its `kind`, `detail`, `line` and the function it is in (`event.func`).
    Event,
    /// A decision document of the project: its name, pack, labels and kind,
    /// and where it is written.
    Decision,
}

impl Select {
    const ALL: [Self; 10] = [
        Self::Symbol,
        Self::Function,
        Self::Edge,
        Self::Module,
        Self::File,
        Self::Test,
        Self::Comment,
        Self::Annotation,
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
            Self::Annotation => "annotation",
            Self::Event => "event",
            Self::Decision => "decision",
        }
    }
}

/// A named CEL value: an intermediate result the check, its message, its
/// evidence and its fix all see by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    /// A lower-case identifier that no expression variable already uses.
    pub name: String,
    /// CEL.
    pub expression: String,
    /// With parameters, the variable is a function of them: `name(a, b)` is
    /// the expression, with its parameters bound to the arguments. It sees the
    /// `options` and the `file` of the caller, and nothing else.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<String>,
}

impl Variable {
    /// What is wrong with the name or the expression, described.
    pub(crate) fn problem(&self, cel: &Env) -> Option<String> {
        let name = self.name.as_str();
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
            && name != "options"
            && name != "node"
            && !Select::ALL.iter().any(|s| s.variable() == name);
        if !valid {
            return Some(format!("check variables: `{name}` is not a usable name"));
        }
        if let Some(param) = self.params.iter().find(|p| {
            p.is_empty()
                || !p
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
                || *p == "options"
                || *p == "file"
        }) {
            return Some(format!(
                "check variable `{name}`: `{param}` is not a usable parameter"
            ));
        }
        compile_problem(cel, &format!("check variable `{name}`"), &self.expression)
    }
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
    pub(crate) fn problem(&self, cel: &Env) -> Option<String> {
        if let Some(problem) = self.variables.iter().find_map(|v| v.problem(cel)) {
            return Some(problem);
        }
        let mut seen = std::collections::BTreeSet::new();
        if let Some(twice) = self
            .variables
            .iter()
            .find(|v| !seen.insert(v.name.as_str()))
        {
            return Some(format!(
                "check variables: `{}` is defined twice",
                twice.name
            ));
        }
        if let Some(problem) = compile_problem(cel, "check where", &self.condition) {
            return Some(problem);
        }
        if let Some(problem) = self.abstain_problem(cel) {
            return Some(problem);
        }
        for (name, source) in &self.evidence {
            if let Some(problem) = compile_problem(cel, &format!("check evidence `{name}`"), source)
            {
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
                source.and_then(|source| compile_problem(cel, &format!("check {what}"), source))
            {
                return Some(problem);
            }
        }
        template_problem(cel, "check message", &self.message)
    }

    /// What is wrong with `abstain` or `prompt`.
    fn abstain_problem(&self, cel: &Env) -> Option<String> {
        if self.abstain.is_none() && self.prompt.is_some() {
            return Some("check: `prompt` needs an `abstain`".to_owned());
        }
        if self.prompt.as_deref().is_some_and(|p| p.trim().is_empty()) {
            return Some("check: `prompt` is empty".to_owned());
        }
        compile_problem(cel, "check abstain", self.abstain.as_deref()?)
    }
}
