//! A `cel` check: a `where` expression that is true for a violation of the
//! selected values, a message and evidence.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

use cel::{Program, common::value::CowVal, context::VariableResolver};
use lighthouse_model::{
    Diagnostic, Position, Reach, Span,
    ucm::{Edge, File, Node, Project, Symbol, SymbolId},
};
use lighthouse_plugin::{Context, Error as PluginError, RuleManifest};
use lighthouse_spec::{CelCheck, Decision, Env, Select};
use serde_json::{Map, Value};

use crate::{
    Composition, Error,
    abstain::{Abstain, Candidate},
    builder::Builder,
    caching,
    eval::{Fact, Frame, Template, cel_fact, compile, compile_all, compile_shared},
    facts,
    library::{self, Needs},
    metrics::MEASURES,
    modules::{self, Declared},
};

/// A CEL check with every expression compiled.
pub(crate) struct CelRule {
    select: Select,
    variables: Vec<(String, Arc<Program>)>,
    condition: Program,
    message: Template,
    evidence: Vec<(String, Program)>,
    at: Option<At>,
    identity: Option<Identity>,
    abstain: Option<Abstain>,
    needs: Needs,
    /// Per variable: whether it reads nothing of the selected value (only the
    /// file, the options and variables like it), so a file has one value of it.
    file_level: Vec<bool>,
    /// Per variable, whether an expression of the check reads it, directly or
    /// through another: the others are not evaluated.
    used: Vec<bool>,
    library: cel::Context<'static, 'static>,
    /// The libraries (id and version) whose functions the expressions call.
    libraries: BTreeMap<String, String>,
}

struct At {
    symbol: Option<Program>,
    line: Option<Program>,
    end_line: Option<Program>,
}

impl At {
    fn compile(cel: &Env, id: &str, at: &lighthouse_spec::At) -> Result<Self, Error> {
        let maybe = |what: &str, source: &Option<String>| optional(cel, id, what, source);
        Ok(Self {
            symbol: maybe("at.symbol", &at.symbol)?,
            line: maybe("at.line", &at.line)?,
            end_line: maybe("at.endLine", &at.end_line)?,
        })
    }
}

struct Identity {
    subject: Program,
    snippet: Option<Program>,
}

impl Identity {
    fn compile(cel: &Env, id: &str, identity: &lighthouse_spec::Identity) -> Result<Self, Error> {
        Ok(Self {
            subject: compile(cel, id, "identity.subject", &identity.subject)?,
            snippet: optional(cel, id, "identity.snippet", &identity.snippet)?,
        })
    }
}

/// Where a finding goes, and what it is about.
#[derive(Clone)]
struct Anchor {
    file: std::path::PathBuf,
    span: Span,
    symbol: Option<SymbolId>,
}

impl CelRule {
    /// Compiles `check` for `decision`. The decision's scope has been checked
    /// against what the check selects when the catalog was validated.
    pub(crate) fn new(
        comp: &Composition,
        decision: &Decision,
        check: &CelCheck,
    ) -> Result<Self, Error> {
        let cel = comp.cel();
        let id = decision.id();
        let select = check
            .selects(decision.scope.subject)
            .ok_or_else(|| Error::invalid(id, "a project decision needs a `select`"))?;
        let mut library = cel.context();
        let declared = library::declare_variables(cel, id, &mut library, &check.variables)?;
        let (values, bodies) = (declared.values, declared.bodies);
        let variables: Vec<(String, Arc<Program>)> = values
            .into_iter()
            .map(|v| {
                Ok((
                    v.name.clone(),
                    compile_shared(cel, id, &format!("variable `{}`", v.name), &v.expression)?,
                ))
            })
            .collect::<Result<_, Error>>()?;
        let level = file_level(&variables);
        let shared = |name: &str| {
            variables
                .iter()
                .zip(&level)
                .any(|((variable, _), file_level)| *file_level && variable == name)
        };
        library::check_scope(cel, id, &declared.functions, &bodies, &shared)?;
        let condition = compile(cel, id, "where", &check.condition)?;
        let message = Template::compile(cel, id, "message", &check.message)?;
        let evidence = compile_all(cel, id, "evidence", &check.evidence)?;
        let at = check
            .at
            .as_ref()
            .map(|at| At::compile(cel, id, at))
            .transpose()?;
        let identity = check
            .identity
            .as_ref()
            .map(|i| Identity::compile(cel, id, i))
            .transpose()?;
        let abstain = Abstain::new(cel, decision, check, select)?;
        let read = variables
            .iter()
            .map(|(_, program)| program.as_ref())
            .chain(bodies.iter().map(Arc::as_ref))
            .chain([&condition])
            .chain(message.programs())
            .chain(evidence.iter().map(|(_, program)| program))
            .chain(
                at.iter()
                    .flat_map(|at| [&at.symbol, &at.line, &at.end_line].into_iter().flatten()),
            )
            .chain(
                identity
                    .iter()
                    .flat_map(|i| [Some(&i.subject), i.snippet.as_ref()].into_iter().flatten()),
            )
            .chain(abstain.iter().map(Abstain::program))
            .collect::<Vec<&Program>>();
        library::check_calls(
            comp,
            id,
            &library,
            &declared.functions,
            read.iter().copied(),
        )?;
        let roots: Vec<&Program> = read[variables.len() + bodies.len()..].to_vec();
        let used = library::reached(&variables, &declared.functions, &bodies, &roots);
        let needs = Needs::reading(read.iter().copied());
        if needs.reads_file_annotations() && select != Select::Annotation {
            return Err(Error::invalid(
                id,
                "`file.annotations` is only available with `select: annotation`",
            ));
        }
        let libraries = library::libraries_called(comp, read.iter().copied());
        Ok(Self {
            libraries,
            select,
            file_level: level,
            used,
            needs,
            abstain,
            variables,
            condition,
            message,
            evidence,
            at,
            identity,
            library,
        })
    }

    /// The id and version of each library the expressions call: part of what
    /// a stored finding depends on.
    pub(crate) fn libraries(&self) -> &BTreeMap<String, String> {
        &self.libraries
    }

    /// Whether the check judges the suppression directives, which are known
    /// once the other rules have run.
    pub(crate) fn after_suppression(&self) -> bool {
        self.select == Select::Annotation
    }

    /// Whether the check has a route that puts subjects to the models.
    pub(crate) fn reviews(&self) -> bool {
        self.abstain.is_some()
    }

    /// The analyzers whose facts the expressions read.
    pub(crate) fn analyzers(&self) -> Vec<String> {
        if self.needs.field("metrics") {
            vec![MEASURES.to_owned()]
        } else {
            Vec::new()
        }
    }

    /// How far the findings about one subject reach: a project decision reads
    /// the whole project, and so does one that reports at a symbol other than
    /// its subject; else what the expressions read.
    pub(crate) fn reach(&self, scope: lighthouse_model::RunScope) -> (Reach, bool) {
        let elsewhere = self.at.as_ref().is_some_and(|at| at.symbol.is_some());
        if scope == lighthouse_model::RunScope::Project || elsewhere {
            return (Reach::Global, false);
        }
        caching::reach(&self.needs)
    }

    pub(crate) fn check(
        &self,
        meta: &RuleManifest,
        ctx: &Context,
        options: &Map<String, Value>,
        focus: Option<&str>,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let builder = Builder::new(ctx, &self.needs)?;
        let reviewing = self.reviews() && ctx.models.asks(&meta.id);
        let run = Evaluation {
            rule: self,
            reviewing,
            meta,
            builder,
            focus,
            candidates: RefCell::default(),
            shared: Shared::new(self, meta.scope, cel_fact(&Value::Object(options.clone()))),
        };
        let mut found = match meta.scope {
            lighthouse_model::RunScope::File => run.check_file(ctx),
            lighthouse_model::RunScope::Project => run.check_project(ctx),
        }?;
        if let Some(abstain) = self.abstain.as_ref().filter(|_| reviewing) {
            found.extend(abstain.settle(ctx, meta, run.candidates.take()));
        }
        Ok(found)
    }
}

struct Evaluation<'a> {
    rule: &'a CelRule,
    /// Whether the `abstain` route is evaluated: the run asks the models of this rule.
    reviewing: bool,
    meta: &'a RuleManifest,
    builder: Builder<'a>,
    /// The language a run of a file with code in several is about: its
    /// symbols are the subjects, and the options are the language's. The
    /// subjects that are no symbol belong to the language of the file.
    focus: Option<&'a str>,
    /// The subjects `abstain` picked, to be put to the models.
    candidates: RefCell<Vec<Candidate>>,
    /// What the judgments of the file read in place.
    shared: Shared<'a>,
}

/// The values of one file that all its judgments read without holding a copy
/// each: the file as a value, for expressions that name `file` while judging
/// something in it, and the file-level variables, found when first read and
/// never when nothing reads them. A frame sees them through its variable
/// resolver.
struct Shared<'a> {
    rule: &'a CelRule,
    options: Fact,
    file: OnceLock<Fact>,
    /// The name of each variable that is file-level; none when the check runs
    /// over the project, which has no file.
    names: Vec<Option<String>>,
    /// Their values, `None` when one could not be found: `failed` says why.
    values: Vec<OnceLock<Option<Fact>>>,
    failed: OnceLock<String>,
}

impl<'a> Shared<'a> {
    fn new(rule: &'a CelRule, scope: lighthouse_model::RunScope, options: Fact) -> Self {
        let per_file = scope == lighthouse_model::RunScope::File;
        Self {
            rule,
            options,
            file: OnceLock::new(),
            failed: OnceLock::new(),
            names: rule
                .variables
                .iter()
                .zip(&rule.file_level)
                .map(|((name, _), level)| (*level && per_file).then(|| name.clone()))
                .collect(),
            values: rule.variables.iter().map(|_| OnceLock::new()).collect(),
        }
    }

    /// The value of file-level variable number `at`, read through this
    /// resolver as the expressions of a judgment read it.
    fn compute(&self, at: usize) -> Option<Fact> {
        let (name, program) = &self.rule.variables[at];
        let mut scope = self.rule.library.new_inner_scope();
        scope.set_variable_resolver(self);
        let value = program
            .execute(&scope)
            .map_err(|e| format!("variable `{name}`: {e}"))
            .and_then(|value| {
                Fact::try_from(value)
                    .map_err(|_| format!("variable `{name}`: the value cannot be held"))
            });
        match value {
            Ok(fact) => Some(fact),
            Err(message) => {
                let _ = self.failed.set(message);
                None
            }
        }
    }
}

impl VariableResolver for Shared<'_> {
    fn resolve<'b>(&'b self, variable: &str) -> Option<CowVal<'b, 'b>> {
        let found = if variable == "options" {
            Some(&self.options)
        } else if variable == "file" {
            self.file.get()
        } else {
            let at = self
                .names
                .iter()
                .position(|name| name.as_deref() == Some(variable))?;
            self.values[at].get_or_init(|| self.compute(at)).as_ref()
        };
        found.map(|fact| CowVal::Borrowed(fact.as_ref()))
    }
}

mod evaluation;

/// Per variable, whether it is file-level: it names no selected value and no
/// variable that does.
pub(crate) fn file_level(variables: &[(String, Arc<Program>)]) -> Vec<bool> {
    let mut per_subject: Vec<&str> = lighthouse_spec::SUBJECT_VARIABLES
        .into_iter()
        .filter(|name| *name != "file")
        .collect();
    variables
        .iter()
        .map(|(name, program)| {
            let names = lighthouse_spec::program_identifiers(program);
            let level = !per_subject.iter().any(|s| names.contains(*s));
            if !level {
                per_subject.push(name);
            }
            level
        })
        .collect()
}

/// Where the findings about the edge at position `at` are reported, when it
/// starts at a subject of the decision's scope: at the site of the edge when
/// the provider reports it, else at its symbol or the top of its file. A
/// module edge is judged by the file it comes from, so a module that declares
/// no symbol is judged too; an edge of no one file goes to the module's anchor.
fn edge_anchor<'p>(
    ctx: &Context<'p>,
    declared: &BTreeMap<&str, Declared<'p>>,
    at: usize,
    edge: &Edge,
) -> Option<Anchor> {
    let project = ctx.project;
    match &edge.from {
        Node::Symbol(id) => {
            let symbol = project
                .symbol(id)
                .filter(|s| ctx.applies.admits_symbol(project, s))?;
            Some(Anchor {
                file: symbol.file.clone(),
                span: edge.site.unwrap_or(symbol.span),
                symbol: None,
            })
        }
        Node::Module(m) => {
            let module = project.module(m);
            if let Some(origin) = project.origin(at) {
                let files: Vec<&File> = project.file(origin).into_iter().collect();
                return module
                    .is_none_or(|module| ctx.applies.admits_module(module, &files))
                    .then(|| Anchor {
                        file: origin.to_owned(),
                        span: edge.site.unwrap_or_else(top_of_file),
                        symbol: None,
                    });
            }
            let declared = declared.get(m.as_str())?;
            module
                .is_none_or(|module| admits_module(ctx, declared, module))
                .then(|| Anchor {
                    file: declared.anchor.file.clone(),
                    span: edge.site.unwrap_or(declared.anchor.span),
                    symbol: None,
                })
        }
    }
}

/// Whether a module is a subject of the decision, by the files that declare it.
fn admits_module(
    ctx: &Context,
    declared: &Declared,
    module: &lighthouse_model::ucm::Module,
) -> bool {
    let files: Vec<&File> = declared
        .files
        .iter()
        .filter_map(|path| ctx.project.file(path))
        .collect();
    ctx.applies.admits_module(module, &files)
}

/// The start of a file, where findings that belong to a whole file go.
fn top_of_file() -> Span {
    let p = Position { line: 1, col: 1 };
    Span { start: p, end: p }
}

/// `source` compiled, when there is one.
fn optional(
    cel: &Env,
    id: &str,
    what: &str,
    source: &Option<String>,
) -> Result<Option<Program>, Error> {
    source
        .as_deref()
        .map(|source| compile(cel, id, what, source))
        .transpose()
}
