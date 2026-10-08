use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use cel::{Context, Program};
use lighthouse_model::{Diagnostic, Fingerprint, Options, Project, Span, Symbol};
use lighthouse_plugin::{Ctx, Error as PluginError, Rule, RuleManifest, Scope};
use lighthouse_spec::{CelCheck, Decision, Select};
use serde_json::{Map, Value};

use crate::{
    Error,
    definition::{Compiled, Piece, compile},
    facts,
};

/// A rule built from a decision and its CEL check.
#[derive(Clone)]
pub(crate) struct DeclarativeRule {
    meta: RuleManifest,
    compiled: Arc<Compiled>,
}

impl DeclarativeRule {
    /// Compiles `check` for `decision`. The decision's scope has been checked
    /// against what the check selects when the catalog was validated; a
    /// declarative rule declares no options: it is tuned by editing its
    /// expression.
    pub(crate) fn new(decision: &Decision, check: &CelCheck) -> Result<Self, Error> {
        let id = decision.id();
        let meta = decision
            .rule_manifest()
            .ok_or_else(|| Error::invalid(id, "the decision has no severity or check"))?;
        if meta.scope != check.select.scope() {
            return Err(Error::invalid(
                id,
                format!(
                    "selects `{}`, which a `{}` decision cannot run over",
                    check.select.name(),
                    decision.scope.subject
                ),
            ));
        }
        Ok(Self {
            compiled: Arc::new(compile(check, id)?),
            meta,
        })
    }

    fn check_file(&self, ctx: &Ctx) -> Result<Vec<Diagnostic>, PluginError> {
        let Some((file, text)) = ctx.file else {
            return Ok(Vec::new());
        };
        let project = ctx.project;
        let mut found = Vec::new();
        match self.compiled.select {
            Select::File => {
                let at = top_of_file();
                let value = facts::file(project, file, text);
                found.extend(self.judge(
                    &value,
                    file.path.clone(),
                    at,
                    &file.path.to_string_lossy(),
                )?);
            }
            Select::Symbol => {
                for symbol in project.symbols_in(&file.path) {
                    let value = facts::symbol(project, symbol);
                    found.extend(self.judge_symbol(symbol, &value)?);
                }
            }
            Select::Function => {
                for symbol in project.symbols_in(&file.path) {
                    if let Some(value) = facts::function(project, symbol) {
                        found.extend(self.judge_symbol(symbol, &value)?);
                    }
                }
            }
            Select::Test => {
                for symbol in project.symbols_in(&file.path) {
                    if let Some(case) = project.test(&symbol.id) {
                        let value = facts::test(project, symbol, case);
                        found.extend(self.judge_symbol(symbol, &value)?);
                    }
                }
            }
            Select::Edge | Select::Module => {}
        }
        Ok(found)
    }

    fn check_project(&self, ctx: &Ctx) -> Result<Vec<Diagnostic>, PluginError> {
        let project = ctx.project;
        let first_file = module_anchors(project);
        let mut found = Vec::new();
        match self.compiled.select {
            Select::Module => {
                for module in &project.modules {
                    let Some((anchor, count)) = first_file.get(module.path.as_str()) else {
                        continue;
                    };
                    let files = project
                        .files
                        .iter()
                        .filter(|f| {
                            project
                                .symbols_in(&f.path)
                                .any(|s| s.id.module() == module.path)
                        })
                        .count();
                    let value = facts::module(module, files, *count);
                    found.extend(self.judge(
                        &value,
                        anchor.file.clone(),
                        top_of_file(),
                        &module.path,
                    )?);
                }
            }
            Select::Edge => {
                for edge in &project.edges {
                    let anchor = match &edge.from {
                        lighthouse_model::Node::Symbol(id) => project.symbol(id),
                        lighthouse_model::Node::Module(m) => {
                            first_file.get(m.as_str()).map(|(s, _)| *s)
                        }
                    };
                    let Some(anchor) = anchor else { continue };
                    let value = facts::edge(project, edge);
                    let key = format!("{:?}->{:?}", edge.from, edge.to);
                    found.extend(self.judge(&value, anchor.file.clone(), anchor.span, &key)?);
                }
            }
            _ => {}
        }
        Ok(found)
    }
    fn judge_symbol(
        &self,
        symbol: &Symbol,
        value: &Value,
    ) -> Result<Option<Diagnostic>, PluginError> {
        let found = self.judge(value, symbol.file.clone(), symbol.span, symbol.id.as_str())?;
        Ok(found.map(|d| Diagnostic {
            symbol: Some(symbol.id.as_str().to_owned()),
            ..d
        }))
    }

    /// Evaluates the rule for one selected value; a finding is reported at `span` of `file`.
    fn judge(
        &self,
        value: &Value,
        file: PathBuf,
        span: Span,
        key: &str,
    ) -> Result<Option<Diagnostic>, PluginError> {
        let mut context = Context::default();
        context
            .add_variable(self.compiled.select.variable(), value)
            .map_err(|e| self.fail(e))?;
        let verdict = self.run(&self.compiled.condition, &context)?;
        if verdict != cel::Value::Bool(true) {
            return Ok(None);
        }
        let mut message = String::new();
        for piece in &self.compiled.message {
            match piece {
                Piece::Text(text) => message.push_str(text),
                Piece::Hole(program) => message.push_str(&render(&self.run(program, &context)?)),
            }
        }
        let mut evidence = Map::new();
        for (name, program) in &self.compiled.evidence {
            evidence.insert(name.clone(), to_json(&self.run(program, &context)?));
        }
        let fingerprint = Fingerprint::of(&self.meta.id, &file.to_string_lossy(), key);
        let mut diagnostic = Diagnostic::new(
            &self.meta.id,
            self.meta.severity,
            message,
            file,
            span,
            fingerprint,
        );
        diagnostic.evidence = Value::Object(evidence);
        Ok(Some(diagnostic))
    }

    fn run(&self, program: &Program, context: &Context) -> Result<cel::Value, PluginError> {
        program.execute(context).map_err(|e| self.fail(e))
    }

    fn fail(&self, message: impl std::fmt::Display) -> PluginError {
        PluginError::Failed(format!("{}: {message}", self.meta.id))
    }
}

impl Rule for DeclarativeRule {
    /// The metadata of the decision the rule was built from.
    fn manifest(&self) -> &RuleManifest {
        &self.meta
    }

    /// Rejects every option: a declarative rule is tuned by editing its expression.
    fn validate(&self, options: &Options) -> Result<(), PluginError> {
        match options.keys().next() {
            None => Ok(()),
            Some(key) => Err(PluginError::Options {
                rule: self.meta.id.clone(),
                message: format!("unknown option `{key}`: a declarative rule has none"),
            }),
        }
    }

    /// Runs once per file or once over the project, following the decision's scope.
    fn check(&self, ctx: &Ctx, _: &Options) -> Result<Vec<Diagnostic>, PluginError> {
        match self.meta.scope {
            Scope::File => self.check_file(ctx),
            Scope::Project => self.check_project(ctx),
        }
    }
}

pub(crate) fn render(value: &cel::Value) -> String {
    match value {
        cel::Value::String(s) => s.to_string(),
        cel::Value::Int(i) => i.to_string(),
        cel::Value::UInt(u) => u.to_string(),
        cel::Value::Float(f) => f.to_string(),
        cel::Value::Bool(b) => b.to_string(),
        cel::Value::Null => "null".to_owned(),
        other => to_json(other).to_string(),
    }
}

pub(crate) fn to_json(value: &cel::Value) -> Value {
    match value {
        cel::Value::String(s) => Value::String(s.to_string()),
        cel::Value::Int(i) => Value::from(*i),
        cel::Value::UInt(u) => Value::from(*u),
        cel::Value::Float(f) => Value::from(*f),
        cel::Value::Bool(b) => Value::Bool(*b),
        cel::Value::List(items) => Value::Array(items.iter().map(to_json).collect()),
        cel::Value::Map(map) => Value::Object(
            map.map
                .iter()
                .map(|(k, v)| (k.to_string(), to_json(v)))
                .collect(),
        ),
        _ => Value::Null,
    }
}

/// The symbol of each module that sorts first by file and position, with the
/// module's symbol count; project-scope findings are reported there.
fn module_anchors(project: &Project) -> BTreeMap<&str, (&Symbol, usize)> {
    let mut anchors: BTreeMap<&str, (&Symbol, usize)> = BTreeMap::new();
    for symbol in &project.symbols {
        let entry = anchors.entry(symbol.id.module()).or_insert((symbol, 0));
        entry.1 += 1;
        if (&symbol.file, symbol.span.start) < (&entry.0.file, entry.0.span.start) {
            entry.0 = symbol;
        }
    }
    anchors
}

/// The start of a file, where findings that belong to a whole file go.
fn top_of_file() -> Span {
    let p = lighthouse_model::Position { line: 1, col: 1 };
    Span { start: p, end: p }
}
