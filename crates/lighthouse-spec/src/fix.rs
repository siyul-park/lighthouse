//! The `fix:` block of a decision: how the finding of its rule is fixed. A
//! fix is data, never code of the rule: its `type` names how the proposal is
//! made (generic operations over the code model, an external command, a
//! model) and every type compiles into a provider of the one `Fixer`
//! interface.

use lighthouse_model::{Safety, Severity, ucm::Capability};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    Cel, DecisionSpec, Point, Program, Programs,
    expression::compile_problem,
    union::{refuse_strays, stray_field},
};

/// How a decision's findings are fixed and how far the fix may be trusted.
/// Which fields a fix may have depends on its `type`; any other is refused.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[schemars(transform = refuse_strays)]
pub struct Fix {
    /// Caps what the fixer may claim: a `safe` proposal under an `unsafe`
    /// decision is downgraded. `safe` is reserved for decisions that author `error`.
    pub safety: Safety,
    /// Provider capabilities the fix needs, besides those its edits imply
    /// ([`Fix::needs`]); without them it is declined.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<Capability>,
    /// Which fix wins when several of one round overlap: higher first.
    /// Ties go by the order of the findings. Default: `0`. A fix that only
    /// restates what others decided, such as a formatter's, is negative, so
    /// that fixes of content win and formatting follows them.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub priority: i32,
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
    #[serde(default)]
    priority: i32,
    #[serde(flatten)]
    kind: FixKind,
}

impl<'de> Deserialize<'de> for Fix {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        const COMMON: [&str; 4] = ["safety", "requires", "priority", "type"];
        let value = Value::deserialize(deserializer)?;
        let table = value
            .as_object()
            .ok_or_else(|| D::Error::custom("a fix is a table"))?;
        let command = command_fields();
        let own: &[&str] = match table.get("type").and_then(Value::as_str) {
            Some("cel") => &["edits", "scope"],
            Some("command") => &command,
            Some("model") => &["prompt", "scope"],
            Some("rpc") => &["params"],
            other => {
                return Err(D::Error::custom(match other {
                    None => "a fix needs a `type`: cel, command, model or rpc".to_owned(),
                    Some(found) => {
                        format!("fix `type` is `{found}`, expected cel, command, model or rpc")
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
            priority,
            kind,
        } = serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(Self {
            safety,
            requires,
            priority,
            kind,
        })
    }
}

impl Fix {
    /// The capabilities the fix needs: the ones it says and, for a CEL fix, the
    /// ones its edits imply, read in `cel`.
    pub fn needs(&self, cel: &Cel) -> Vec<Capability> {
        match &self.kind {
            FixKind::Cel { edits, .. } => with_derived(cel, self.requires.clone(), edits),
            _ => self.requires.clone(),
        }
    }

    /// What is wrong with the fix of `decision`, for the catalog to refuse.
    pub(crate) fn problem(&self, cel: &Cel, decision: &DecisionSpec) -> Option<String> {
        if decision.check.is_none() {
            return Some("a decision without a check has no findings to fix".to_owned());
        }
        if self.safety == Safety::Safe && decision.severity != Some(Severity::Error) {
            return Some(
                "`safe` is reserved for decisions that author `error`; use `unsafe`".to_owned(),
            );
        }
        match &self.kind {
            FixKind::Cel { edits, .. } => compile_problem(cel, "fix edits", edits),
            FixKind::Command(command) => command.problem(),
            FixKind::Model { .. } if self.safety == Safety::Safe => {
                Some("a fix by a model is never `safe`; use `unsafe`".to_owned())
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

/// How far the change of a fix may go, which the verifier checks. Ordered
/// from the narrowest. One model for every type of fix: a fix by CEL, by a
/// command and by a model are held to it the same way.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum FixScope {
    /// Only inside the symbol of the finding (its extent): the symbol it is
    /// about, else the innermost one around it, else a short file as a whole.
    Symbol,
    /// Only the file of the finding. What a fix by a command has when it
    /// declares no scope.
    File,
    /// The file of the finding and the files it reaches: a file may change
    /// when it holds a reference site (a call, a use, an import) of a symbol
    /// changed in a file that may change, transitively. A signature changes
    /// with its callers, and a caller whose own signature changes, with its
    /// callers. What a fix by CEL or by a model has when it declares no scope.
    #[default]
    References,
    /// Any file the analysis indexes in the run's scope, for a change that
    /// is not about references, such as the consistency of a doc and a spec.
    Files,
}

impl FixScope {
    /// The scope of the fix of a decision whose `fix` is `fix`: the narrower
    /// of the `cap` of the project and what the decision declares, which for
    /// a command is its `scope`, else `file`, and for CEL and a model its
    /// `scope`, else `references`. Nobody widens. The project's cap is for
    /// fixes by a model: callers ask for the others with the widest cap.
    pub fn of_fix(fix: Option<&Fix>, cap: Self) -> Self {
        match fix.map(|f| &f.kind) {
            Some(FixKind::Model { scope, .. } | FixKind::Cel { scope, .. }) => {
                scope.unwrap_or(Self::References).min(cap)
            }
            Some(FixKind::Command(command)) => command.scope.unwrap_or(Self::File).min(cap),
            _ => cap,
        }
    }
}

impl std::fmt::Display for FixScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Symbol => "symbol",
            Self::File => "file",
            Self::References => "references",
            Self::Files => "files",
        })
    }
}

/// The type of a fix; a `fix:` block holds exactly one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FixKind {
    /// One CEL expression that returns a list of edits, built with `remove`,
    /// `add`, `replace`, `move`, `rename` and `reorder`. It sees `finding`,
    /// `node` (the finding's symbol), `file`, `options` and the check's
    /// `variables`. An empty list proposes no edit.
    Cel {
        edits: String,
        /// How far the edits may go; see [`FixScope`]. Default: `references`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<FixScope>,
    },
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
        /// How far the agent's change may go. Undeclared, the decision limits
        /// nothing and the project's `[fix] scope` decides; declared, it
        /// narrows that scope and the project never widens it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<FixScope>,
    },
    /// A language plugin's fix method. Reserved: not yet supported.
    Rpc {
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        params: Map<String, Value>,
    },
}

/// An external program that fixes a finding, by the process contract. Its
/// `output` says what the result is: `files`, the files it changed in a
/// scratch copy of the project (a repository of one commit, so tools that need
/// one find their root), or `stdout`, the new text of `{file}`, which needs no
/// copy. A result with no change declines the finding, with stderr as the
/// reason; a code outside `success`, a signal or a timeout is an error that
/// applies nothing. stdout carries only the result; stderr is for people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandSpec {
    #[serde(flatten)]
    pub program: Program,
    /// How far the program's change may go. Default: `file`, the file of the
    /// finding; `symbol` and `files` are what a fix by a model also has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<FixScope>,
}

impl CommandSpec {
    /// The programs of the fix: its own and the ones `decision`'s languages
    /// give.
    pub fn programs(&self, decision: &DecisionSpec) -> Programs {
        let languages = decision
            .languages
            .iter()
            .filter_map(|(id, l)| Some((id.clone(), l.fix.clone()?)))
            .collect();
        Programs::new(&self.program, languages)
    }

    /// Why the command cannot run as written.
    fn problem(&self) -> Option<String> {
        let own = self.program.problem(Point::Fix);
        own.filter(|_| !self.program.command.is_empty())
            .or_else(|| {
                let filter = self.program.output(Point::Fix) == crate::Output::Stdout;
                let wide = self.scope.is_some_and(|scope| scope != FixScope::File);
                (filter && wide).then(|| {
                    "`output: stdout` is the new text of one file: its `scope` is `file`".to_owned()
                })
            })
    }
}

/// `requires` with what the edit constructors `edits` calls need: a node that
/// is removed, replaced, moved, reordered or that anchors an insertion needs its
/// extent, and a rename needs every reference of the symbol. A removal or a
/// replacement needs the extent when it names the node itself (`node`,
/// `finding.symbol`): a range of text held in the evidence is for the fix to
/// declare, and the edit declines when the extent is missing.
fn with_derived(cel: &Cel, mut requires: Vec<Capability>, edits: &str) -> Vec<Capability> {
    let mut calls = Vec::new();
    if let Ok(program) = cel.compile(edits) {
        collect(program.expression(), &mut calls);
    }
    let on_node = |(name, node): &&(String, bool)| match name.as_str() {
        "move" | "reorder" | "before" | "after" => true,
        "remove" | "replace" => *node,
        _ => false,
    };
    let derived = [
        (Capability::Extent, calls.iter().any(|c| on_node(&c))),
        (
            Capability::CompleteReferences,
            calls.iter().any(|(name, _)| name == "rename"),
        ),
    ];
    for (capability, needed) in derived {
        if needed && !requires.contains(&capability) {
            requires.push(capability);
        }
    }
    requires
}

/// The calls in `expr`, each with whether its first argument is a node: a
/// variable (`node`, `func`) or the `symbol` of the finding.
fn collect(expr: &cel::common::ast::IdedExpr, calls: &mut Vec<(String, bool)>) {
    use cel::common::ast::Expr;
    if let Expr::Call(call) = &expr.expr {
        let node = call.args.first().is_some_and(|first| match &first.expr {
            Expr::Ident(_) => true,
            Expr::Select(select) => select.field == "symbol",
            _ => false,
        });
        calls.push((call.func_name.clone(), node));
    }
    for child in crate::children(expr) {
        collect(child, calls);
    }
}

/// The keys a `command` fix may have besides the common ones.
fn command_fields() -> Vec<&'static str> {
    let mut own = crate::program::FIELDS.to_vec();
    own.push("scope");
    own
}

fn is_zero(priority: &i32) -> bool {
    *priority == 0
}
