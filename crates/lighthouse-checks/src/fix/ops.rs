//! The generic operations of an `ops` fix: CEL over the finding, lowered to
//! IR edit operations. Semantics that need Rust (`reorder`) live here; the
//! text edits themselves are the orchestrator's.

use std::path::PathBuf;

use cel::{Context, Program};
use lighthouse_model::{Anchor, EditOp, Node, Owner, Project, Span, Symbol, SymbolId, SymbolKind};
use lighthouse_plugin::{Error as PluginError, FixRequest, KeyCtx};
use lighthouse_spec::{OpSpec, ReorderScope};
use serde_json::{Value, json};

use crate::{
    Error,
    eval::Template,
    facts,
    rule::{render, to_json},
};

/// The result of evaluating: edit operations, or the reason none apply.
pub(crate) type Evaluated = Result<Vec<EditOp>, String>;

enum Step {
    Move {
        node: Program,
        anchor: Program,
        before: bool,
    },
    Reorder {
        scope: ReorderScope,
        by: Vec<String>,
    },
    DeleteNode(Program),
    DeleteRange {
        file: Program,
        span: Program,
    },
    Rename {
        symbol: Program,
        name: Template,
    },
    Replace {
        file: Program,
        span: Program,
        text: Template,
    },
}

/// The compiled operations of one fix, each with its guard.
pub(crate) struct Steps(Vec<(Option<Program>, Step)>);

impl Steps {
    pub(crate) fn compile(id: &str, list: &[OpSpec]) -> Result<Self, Error> {
        let program = |what: &str, source: &str| {
            Program::compile(source).map_err(|e| Error::invalid(id, format!("fix {what}: {e}")))
        };
        let mut steps = Vec::new();
        for op in list {
            let when = op.when().map(|w| program("when", w)).transpose()?;
            let step = match op {
                OpSpec::Move {
                    node,
                    before,
                    after,
                    ..
                } => {
                    let (target, is_before) = match (before, after) {
                        (Some(b), None) => (b, true),
                        (None, Some(a)) => (a, false),
                        _ => return Err(Error::invalid(id, "fix move needs before or after")),
                    };
                    Step::Move {
                        node: program("move node", node)?,
                        anchor: program("move anchor", target)?,
                        before: is_before,
                    }
                }
                OpSpec::Reorder { scope, by, .. } => Step::Reorder {
                    scope: *scope,
                    by: by.clone(),
                },
                OpSpec::Delete {
                    node, file, span, ..
                } => match (node, file, span) {
                    (Some(node), None, None) => Step::DeleteNode(program("delete node", node)?),
                    (None, Some(file), Some(span)) => Step::DeleteRange {
                        file: program("delete file", file)?,
                        span: program("delete span", span)?,
                    },
                    _ => {
                        return Err(Error::invalid(
                            id,
                            "fix delete needs node, or file and span",
                        ));
                    }
                },
                OpSpec::Rename { symbol, name, .. } => Step::Rename {
                    symbol: program("rename symbol", symbol)?,
                    name: Template::compile(id, "fix template", name)?,
                },
                OpSpec::Replace {
                    file, span, text, ..
                } => Step::Replace {
                    file: program("replace file", file)?,
                    span: program("replace span", span)?,
                    text: Template::compile(id, "fix template", text)?,
                },
            };
            steps.push((when, step));
        }
        Ok(Self(steps))
    }

    /// Evaluates every operation whose guard holds. Declined when none does.
    pub(crate) fn evaluate(
        &self,
        id: &str,
        request: &FixRequest,
    ) -> Result<Evaluated, PluginError> {
        let env = Env::new(id, request)?;
        let mut out = Vec::new();
        for (when, step) in &self.0 {
            if let Some(when) = when {
                match env.run(when)? {
                    cel::Value::Bool(true) => {}
                    cel::Value::Bool(false) => continue,
                    other => {
                        return Err(env.fail(format!("`when` is {}, not a bool", render(&other))));
                    }
                }
            }
            match env.step(step, request)? {
                Ok(ops) => out.extend(ops),
                Err(reason) => return Ok(Err(reason)),
            }
        }
        if out.is_empty() {
            return Ok(Err(
                "no operation of the fix applies to this finding".to_owned()
            ));
        }
        Ok(Ok(out))
    }
}

/// What expressions see: the finding, its symbol and the rule's options.
struct Env<'a> {
    id: &'a str,
    context: Context<'static>,
}

impl<'a> Env<'a> {
    fn new(id: &'a str, request: &FixRequest) -> Result<Self, PluginError> {
        let finding = request.finding;
        let evidence = if finding.evidence.is_null() {
            json!({})
        } else {
            finding.evidence.clone()
        };
        let value = json!({
            "rule": finding.rule_id,
            "message": finding.message,
            "file": finding.file.to_string_lossy().replace('\\', "/"),
            "span": finding.span,
            "symbol": finding.symbol.clone().unwrap_or_default(),
            "evidence": evidence,
            "fingerprint": finding.fingerprint.as_str(),
            "facts": request.facts,
        });
        let symbol = finding
            .symbol
            .as_deref()
            .and_then(SymbolId::parse)
            .and_then(|id| request.project.symbol(&id))
            .map_or_else(|| json!({}), |s| facts::symbol(request.project, s));
        let mut context = Context::default();
        for (name, value) in [
            ("finding", &value),
            ("symbol", &symbol),
            ("options", &Value::Object(request.options.clone())),
        ] {
            context
                .add_variable(name, value)
                .map_err(|e| PluginError::Failed(format!("{id}: fix: {e}")))?;
        }
        Ok(Self { id, context })
    }

    fn run(&self, program: &Program) -> Result<cel::Value, PluginError> {
        program.execute(&self.context).map_err(|e| self.fail(e))
    }

    fn fail(&self, message: impl std::fmt::Display) -> PluginError {
        PluginError::Failed(format!("{}: fix: {message}", self.id))
    }

    fn step(&self, step: &Step, request: &FixRequest) -> Result<Evaluated, PluginError> {
        Ok(Ok(match step {
            Step::Move {
                node,
                anchor,
                before,
            } => {
                let node = self.node(node, "node")?;
                let target = self.node(anchor, "before/after")?;
                vec![EditOp::Move {
                    node,
                    anchor: if *before {
                        Anchor::Before(target)
                    } else {
                        Anchor::After(target)
                    },
                }]
            }
            Step::Reorder { scope, by } => return self.reorder(*scope, by, request),
            Step::DeleteNode(node) => vec![EditOp::Delete {
                node: self.node(node, "node")?,
            }],
            Step::DeleteRange { file, span } => vec![EditOp::DeleteRange {
                file: PathBuf::from(self.string(file, "file")?),
                span: self.span(span, "span")?,
            }],
            Step::Rename { symbol, name } => {
                let Node::Symbol(symbol) = self.node(symbol, "symbol")? else {
                    return Err(self.fail("`symbol` is not a symbol id"));
                };
                vec![EditOp::Rename {
                    symbol,
                    name: self.text(name)?,
                }]
            }
            Step::Replace { file, span, text } => vec![EditOp::Replace {
                file: PathBuf::from(self.string(file, "file")?),
                span: self.span(span, "span")?,
                text: self.text(text)?,
            }],
        }))
    }

    fn string(&self, program: &Program, what: &str) -> Result<String, PluginError> {
        match self.run(program)? {
            cel::Value::String(s) => Ok(s.to_string()),
            other => Err(self.fail(format!("`{what}` is {}, not a string", render(&other)))),
        }
    }

    fn node(&self, program: &Program, what: &str) -> Result<Node, PluginError> {
        let text = self.string(program, what)?;
        SymbolId::parse(&text)
            .map(Node::Symbol)
            .ok_or_else(|| self.fail(format!("`{what}` is `{text}`, not a symbol id")))
    }

    fn span(&self, program: &Program, what: &str) -> Result<Span, PluginError> {
        let value = to_json(&self.run(program)?);
        serde_json::from_value(value.clone())
            .map_err(|_| self.fail(format!("`{what}` is {value}, not a span")))
    }

    fn text(&self, template: &Template) -> Result<String, PluginError> {
        template.render(|program| self.run(program))
    }

    /// Orders the declarations of the finding's file by the keys. Declarations
    /// the first key does not order stay in their places; the orchestrator
    /// permutes each container on its own.
    fn reorder(
        &self,
        scope: ReorderScope,
        by: &[String],
        request: &FixRequest,
    ) -> Result<Evaluated, PluginError> {
        let project = request.project;
        let Some(anchor) = request
            .finding
            .symbol
            .as_deref()
            .and_then(SymbolId::parse)
            .and_then(|id| project.symbol(&id))
        else {
            return Ok(Err("the finding has no symbol to reorder around".to_owned()));
        };
        let language = project.file(&anchor.file).map_or("", |f| f.lang.as_str());
        let ctx = KeyCtx {
            project,
            language,
            rule: &request.finding.rule_id,
            options: request.options,
            constructors: request.ws.constructor_prefixes(language),
        };
        let mut keys = Vec::new();
        for id in by {
            keys.push(
                request
                    .keys
                    .key(id)
                    .ok_or_else(|| self.fail(format!("unknown order key `{id}`")))?,
            );
        }
        let mut members: Vec<&Symbol> = project
            .symbols_in(&anchor.file)
            .filter(|s| orderable(project, s))
            .filter(|s| scope == ReorderScope::File || s.owner == anchor.owner)
            .collect();
        members.sort_by_key(|s| (s.extent.map(|e| e.start), s.id.clone()));
        let mut ranked: Vec<(Vec<u64>, usize)> = Vec::new();
        for (at, symbol) in members.iter().enumerate() {
            let mut ranks = Vec::new();
            for (n, key) in keys.iter().enumerate() {
                match key.rank(&ctx, symbol)? {
                    Some(rank) => ranks.push(rank),
                    None if n == 0 => break,
                    None => ranks.push(u64::MAX),
                }
            }
            if ranks.len() == keys.len() {
                ranked.push((ranks, at));
            }
        }
        if ranked.len() < 2 {
            return Ok(Err(
                "fewer than two declarations are ordered by the keys".to_owned()
            ));
        }
        let source: Vec<usize> = ranked.iter().map(|(_, at)| *at).collect();
        ranked.sort();
        let order: Vec<usize> = ranked.iter().map(|(_, at)| *at).collect();
        if order == source {
            return Ok(Err(
                "the declarations are already in the order the keys ask for".to_owned(),
            ));
        }
        let owner = match (scope, &anchor.owner) {
            (ReorderScope::Owner, Some(owner)) => Owner::Symbol(owner.clone()),
            _ => Owner::File(anchor.file.clone()),
        };
        Ok(Ok(vec![EditOp::Reorder {
            owner,
            order: order
                .into_iter()
                .map(|at| Node::Symbol(members[at].id.clone()))
                .collect(),
        }]))
    }
}

/// A declaration a reorder may move: it has an extent and is not a field,
/// nor a member of anything but a type.
fn orderable(project: &Project, symbol: &Symbol) -> bool {
    let member_of_type = |owner: &SymbolId| {
        project
            .symbol(owner)
            .is_some_and(|o| o.kind == SymbolKind::Type)
            && matches!(
                symbol.kind,
                SymbolKind::Method | SymbolKind::Const | SymbolKind::Var
            )
    };
    symbol.extent.is_some()
        && !matches!(symbol.kind, SymbolKind::Field | SymbolKind::Variant)
        && symbol.owner.as_ref().is_none_or(member_of_type)
}
