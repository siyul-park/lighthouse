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
use lighthouse_spec::{Cel, CelCheck, Decision, Select};
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
    fn compile(cel: &Cel, id: &str, at: &lighthouse_spec::At) -> Result<Self, Error> {
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
    fn compile(cel: &Cel, id: &str, identity: &lighthouse_spec::Identity) -> Result<Self, Error> {
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

impl Evaluation<'_> {
    fn check_file(&self, ctx: &Context) -> Result<Vec<Diagnostic>, PluginError> {
        let Some((file, text)) = ctx.file else {
            return Ok(Vec::new());
        };
        if self.rule.select != Select::File && self.rule.needs.ident("file") {
            let _ = self
                .shared
                .file
                .set(cel_fact(&self.builder.file(file, text)));
        }
        let symbols = matches!(
            self.rule.select,
            Select::Symbol | Select::Function | Select::Test | Select::Event
        );
        if !symbols && self.focus.is_some_and(|l| l != file.analysis_language()) {
            return Ok(Vec::new());
        }
        match self.rule.select {
            Select::File if ctx.applies.admits_file(file) => {
                let value = cel_fact(&self.builder.file(file, text));
                let _ = self.shared.file.set(value.clone_as_boxed());
                let anchor = Anchor {
                    file: file.path.clone(),
                    span: top_of_file(),
                    symbol: None,
                };
                Ok(self
                    .judge(value, anchor, &file.path.to_string_lossy())?
                    .into_iter()
                    .collect())
            }
            Select::Symbol | Select::Function | Select::Test | Select::Event => {
                self.check_symbols(ctx, file)
            }
            Select::Comment => self.check_comments(ctx, file, text),
            Select::Annotation => self.check_annotations(file),
            Select::File | Select::Edge | Select::Module | Select::Decision => Ok(Vec::new()),
        }
    }

    /// The symbols of `file` that are subjects of the decision's scope.
    fn check_symbols(&self, ctx: &Context, file: &File) -> Result<Vec<Diagnostic>, PluginError> {
        let project = ctx.project;
        let mut found = Vec::new();
        for symbol in project
            .symbols_in(&file.path)
            .filter(|s| ctx.applies.admits_symbol(project, s))
            .filter(|s| {
                self.focus
                    .is_none_or(|language| project.language_of(s) == Some(language))
            })
        {
            if self.rule.select == Select::Event {
                found.extend(self.judge_events(symbol)?);
                continue;
            }
            let value = match self.rule.select {
                Select::Function => self.builder.function_fact(symbol),
                Select::Test => self.builder.test_fact(symbol),
                _ => Some(self.builder.symbol_fact(symbol)),
            };
            if let Some(value) = value {
                found.extend(self.judge_symbol(symbol, value)?);
            }
        }
        Ok(found)
    }

    /// One finding per event of the function, at the event.
    fn judge_events(&self, function: &Symbol) -> Result<Vec<Diagnostic>, PluginError> {
        let mut found = Vec::new();
        for event in self.builder.event_facts(function) {
            let place = Anchor {
                file: function.file.clone(),
                span: event.span,
                symbol: Some(function.id.clone()),
            };
            found.extend(self.judge(event.fact, place, &event.key)?);
        }
        Ok(found)
    }

    /// The comments of `file` that are subjects of the decision's scope.
    fn check_comments(
        &self,
        ctx: &Context,
        file: &File,
        text: &str,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let project = ctx.project;
        let mut found = Vec::new();
        for comment in project.comments_in(&file.path) {
            if !ctx.applies.admits_comment(project.file(&comment.file)) {
                continue;
            }
            let value = cel_fact(&self.builder.comment(comment, text));
            let place = Anchor {
                file: file.path.clone(),
                span: comment.span,
                symbol: None,
            };
            found.extend(self.judge(value, place, &comment.text)?);
        }
        Ok(found)
    }

    /// The directives of `file`, one subject per rule each names, as the
    /// engine judged them.
    fn check_annotations(&self, file: &File) -> Result<Vec<Diagnostic>, PluginError> {
        let Some(annotations) = self.builder.annotations(&file.path) else {
            return Err(PluginError::Failed(format!(
                "{}: the directives are judged after the other rules, and this rule ran before",
                self.meta.id
            )));
        };
        let mut found = Vec::new();
        for annotation in annotations {
            let span = serde_json::from_value(annotation["place"].clone()).map_err(|e| {
                PluginError::Failed(format!("{}: an annotation has no place: {e}", self.meta.id))
            })?;
            let place = Anchor {
                file: file.path.clone(),
                span,
                symbol: None,
            };
            let key = annotation["snippet"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            found.extend(self.judge(cel_fact(&annotation), place, &key)?);
        }
        Ok(found)
    }

    fn check_project(&self, ctx: &Context) -> Result<Vec<Diagnostic>, PluginError> {
        let project = ctx.project;
        let declared = modules::index(project);
        let mut found = Vec::new();
        match self.rule.select {
            Select::Module => {
                for module in &project.modules {
                    let Some(declared) = declared.get(module.path.as_str()) else {
                        continue;
                    };
                    if !admits_module(ctx, declared, module) {
                        continue;
                    }
                    let value = cel_fact(&self.builder.module(module, declared));
                    let place = Anchor {
                        file: declared.anchor.file.clone(),
                        span: top_of_file(),
                        symbol: None,
                    };
                    found.extend(self.judge(value, place, &module.path)?);
                }
            }
            Select::Edge => {
                for (at, edge) in project.edges.iter().enumerate() {
                    let Some(place) = edge_anchor(ctx, &declared, at, edge) else {
                        continue;
                    };
                    let value = cel_fact(&facts::edge(project, edge));
                    let key = format!("{:?}->{:?}", edge.from, edge.to);
                    found.extend(self.judge(value, place, &key)?);
                }
            }
            Select::Decision => found.extend(self.check_decisions(project)?),
            _ => {}
        }
        Ok(found)
    }

    /// The decision documents of the project that are subjects.
    fn check_decisions(&self, project: &Project) -> Result<Vec<Diagnostic>, PluginError> {
        let mut found = Vec::new();
        for document in project.documents().iter().filter(|d| d.kind == "Decision") {
            let value = cel_fact(&facts::document(document));
            let place = Anchor {
                file: document.file.clone(),
                span: Span {
                    start: document.at,
                    end: document.at,
                },
                symbol: None,
            };
            // A decision is the same one under another name.
            let key = document.uid.as_deref().unwrap_or(&document.name);
            found.extend(self.judge(value, place, key)?);
        }
        Ok(found)
    }

    fn judge_symbol(
        &self,
        symbol: &Symbol,
        value: Fact,
    ) -> Result<Option<Diagnostic>, PluginError> {
        let place = Anchor {
            file: symbol.file.clone(),
            span: symbol.span,
            symbol: Some(symbol.id.clone()),
        };
        self.judge(value, place, symbol.id.as_str())
    }

    /// Evaluates the rule for one selected value; a finding is reported at
    /// `place` unless the check says otherwise. A variable of the file that
    /// could not be found is the failure, not the expression that read it.
    fn judge(
        &self,
        value: Fact,
        place: Anchor,
        key: &str,
    ) -> Result<Option<Diagnostic>, PluginError> {
        self.judge_value(value, place, key).map_err(|error| {
            self.shared.failed.get().map_or(error, |message| {
                PluginError::Incomplete(format!("{}: execution error: {message}", self.meta.id))
            })
        })
    }

    fn judge_value(
        &self,
        value: Fact,
        place: Anchor,
        key: &str,
    ) -> Result<Option<Diagnostic>, PluginError> {
        let rule = self.rule;
        let mut frame = Frame::new(&self.meta.id, &rule.library);
        frame.set_fact(rule.select.variable(), value);
        frame.share(&self.shared);
        for (at, (name, program)) in rule.variables.iter().enumerate() {
            if rule.used[at] {
                self.variable(&mut frame, at, name, program)?;
            }
        }
        let found = frame.truth(&rule.condition)?;
        let abstain = rule.abstain.as_ref().filter(|_| self.reviewing);
        match (abstain, found) {
            (None, false) => return Ok(None),
            (Some(abstain), false) if !abstain.asks(&frame)? => return Ok(None),
            _ => {}
        }
        // The message and evidence of a subject only suspected may read what
        // the subject does not have: they are then left empty.
        let message = frame.text(&rule.message);
        let evidence = frame.evidence(&rule.evidence);
        let (message, evidence) = if found {
            (message?, evidence?)
        } else {
            (message.unwrap_or_default(), evidence.unwrap_or_default())
        };
        // Like the message, the place of a subject only suspected may name what
        // the subject does not have: it is then the default.
        let place = match self.place(&frame, place.clone()) {
            Err(error) if found => return Err(error),
            placed => placed.unwrap_or(place),
        };
        let fingerprint = match &rule.identity {
            Some(identity) => {
                let subject = frame.string(&identity.subject, "identity.subject")?;
                let snippet = match &identity.snippet {
                    Some(program) => frame.string(program, "identity.snippet")?,
                    None => String::new(),
                };
                self.meta.fingerprint(&subject, &snippet)
            }
            None => self.meta.fingerprint(&place.file.to_string_lossy(), key),
        };
        let mut diagnostic = Diagnostic::new(
            &self.meta.id,
            self.meta.severity,
            message,
            place.file,
            place.span,
            fingerprint,
        );
        diagnostic.symbol = place.symbol.map(|id| id.as_str().to_owned());
        diagnostic.evidence = evidence;
        if let Some(abstain) = abstain {
            let fact = abstain.fact_of(&frame)?;
            self.candidates.borrow_mut().push(Candidate {
                diagnostic: diagnostic.clone(),
                fact,
                decided: found,
            });
            if !found {
                return Ok(None);
            }
        }
        Ok(Some(diagnostic))
    }

    /// Gives variable number `at` its value in the frame: the file's one
    /// value, found once, when it reads nothing of the subject.
    fn variable(
        &self,
        frame: &mut Frame,
        at: usize,
        name: &str,
        program: &Program,
    ) -> Result<(), PluginError> {
        let value = |frame: &Frame| {
            Fact::try_from(frame.run(program)?).map_err(|_| frame.fail("the value cannot be held"))
        };
        // A file-level variable is not held by the frame: whatever reads it
        // finds it through the resolver, when it first does.
        if self.shared.names[at].is_none() {
            frame.set_fact(name, value(frame)?);
        }
        Ok(())
    }

    fn place(&self, frame: &Frame, default: Anchor) -> Result<Anchor, PluginError> {
        let Some(at) = &self.rule.at else {
            return Ok(default);
        };
        if let Some(program) = &at.symbol {
            let text = frame.string(program, "at.symbol")?;
            let symbol = SymbolId::parse(&text)
                .and_then(|id| self.builder.project().symbol(&id))
                .ok_or_else(|| {
                    frame.fail(format!(
                        "`at.symbol` is `{text}`, not a symbol of the project"
                    ))
                })?;
            return Ok(Anchor {
                file: symbol.file.clone(),
                span: symbol.span,
                symbol: Some(symbol.id.clone()),
            });
        }
        let line = |program: &Option<Program>, what: &str| -> Result<Option<u32>, PluginError> {
            program
                .as_ref()
                .map(|p| Ok(u32::try_from(frame.int(p, what)?.max(1)).unwrap_or(u32::MAX)))
                .transpose()
        };
        let start = line(&at.line, "at.line")?;
        let end = line(&at.end_line, "at.endLine")?;
        let position = |line: u32| Position { line, col: 1 };
        let span = match (start, end) {
            (None, None) => default.span,
            (Some(s), None) => Span {
                start: position(s),
                end: position(s),
            },
            (None, Some(e)) => Span {
                start: default.span.start,
                end: position(e),
            },
            (Some(s), Some(e)) => Span {
                start: position(s),
                end: position(e),
            },
        };
        Ok(Anchor { span, ..default })
    }
}

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
    cel: &Cel,
    id: &str,
    what: &str,
    source: &Option<String>,
) -> Result<Option<Program>, Error> {
    source
        .as_deref()
        .map(|source| compile(cel, id, what, source))
        .transpose()
}
