//! A `cel` check: a `where` expression that is true for a violation of the
//! selected values, a message and evidence.

use std::collections::{BTreeMap, BTreeSet};

use cel::Program;
use lighthouse_model::{Diagnostic, File, Node, Position, Project, Reach, Span, Symbol, SymbolId};
use lighthouse_plugin::{Ctx, Error as PluginError, RuleManifest};
use lighthouse_spec::{CelCheck, Decision, Select};
use serde_json::{Map, Value};

use crate::{
    Error,
    builder::{Builder, METRIC_ANALYZERS},
    eval::{Fact, Frame, Template, cel_fact, compile, compile_all},
    facts,
    library::{self, Needs},
    modules, reach,
};

/// A CEL check with every expression compiled.
pub(crate) struct CelRule {
    select: Select,
    bindings: Vec<(String, Program)>,
    condition: Program,
    message: Template,
    evidence: Vec<(String, Program)>,
    at: Option<At>,
    identity: Option<Identity>,
    needs: Needs,
    library: cel::Context<'static>,
}

struct At {
    symbol: Option<Program>,
    line: Option<Program>,
    end_line: Option<Program>,
}

struct Identity {
    subject: Program,
    snippet: Option<Program>,
}

/// Where a finding goes, and what it is about.
struct Place {
    file: std::path::PathBuf,
    span: Span,
    symbol: Option<SymbolId>,
}

impl CelRule {
    /// Compiles `check` for `decision`. The decision's scope has been checked
    /// against what the check selects when the catalog was validated.
    pub(crate) fn new(decision: &Decision, check: &CelCheck) -> Result<Self, Error> {
        let id = decision.id();
        let mut sources: Vec<&str> = vec![&check.condition, &check.message];
        sources.extend(check.bindings.iter().map(|b| b.expr.as_str()));
        sources.extend(check.evidence.values().map(String::as_str));
        if let Some(at) = &check.at {
            for source in [&at.symbol, &at.line, &at.end_line] {
                sources.extend(source.as_deref());
            }
        }
        if let Some(identity) = &check.identity {
            sources.push(&identity.subject);
            sources.extend(identity.snippet.as_deref());
        }
        let maybe = |what: &str, source: &Option<String>| {
            source.as_deref().map(|s| compile(id, what, s)).transpose()
        };
        Ok(Self {
            select: check
                .selects(decision.scope.subject)
                .ok_or_else(|| Error::invalid(id, "a project decision needs a `select`"))?,
            bindings: check
                .bindings
                .iter()
                .map(|b| {
                    Ok((
                        b.name.clone(),
                        compile(id, &format!("with `{}`", b.name), &b.expr)?,
                    ))
                })
                .collect::<Result<_, Error>>()?,
            condition: compile(id, "where", &check.condition)?,
            message: Template::compile(id, "message", &check.message)?,
            evidence: compile_all(id, "evidence", &check.evidence)?,
            at: check
                .at
                .as_ref()
                .map(|at| {
                    Ok::<_, Error>(At {
                        symbol: maybe("at.symbol", &at.symbol)?,
                        line: maybe("at.line", &at.line)?,
                        end_line: maybe("at.endLine", &at.end_line)?,
                    })
                })
                .transpose()?,
            identity: check
                .identity
                .as_ref()
                .map(|i| {
                    Ok::<_, Error>(Identity {
                        subject: compile(id, "identity.subject", &i.subject)?,
                        snippet: maybe("identity.snippet", &i.snippet)?,
                    })
                })
                .transpose()?,
            needs: Needs::of(sources),
            library: library::context(),
        })
    }

    /// The analyzers whose facts the expressions read.
    pub(crate) fn analyzers(&self) -> Vec<String> {
        if self.needs.function("metrics") {
            METRIC_ANALYZERS.iter().map(|a| (*a).to_owned()).collect()
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
        reach::of(&self.needs)
    }

    pub(crate) fn check(
        &self,
        meta: &RuleManifest,
        ctx: &Ctx,
        options: &Map<String, Value>,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let builder = Builder::new(ctx, &self.needs, options, &meta.id)?;
        let run = Run {
            rule: self,
            meta,
            builder,
            options: cel_fact(&Value::Object(options.clone())),
        };
        match meta.scope {
            lighthouse_model::RunScope::File => run.check_file(ctx),
            lighthouse_model::RunScope::Project => run.check_project(ctx),
        }
    }
}

struct Run<'a> {
    rule: &'a CelRule,
    meta: &'a RuleManifest,
    builder: Builder<'a>,
    options: Fact,
}

impl Run<'_> {
    fn check_file(&self, ctx: &Ctx) -> Result<Vec<Diagnostic>, PluginError> {
        let Some((file, text)) = ctx.file else {
            return Ok(Vec::new());
        };
        match self.rule.select {
            Select::File if ctx.applies.admits_file(file) => {
                let value = cel_fact(&self.builder.file(file, text));
                let anchor = Place {
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
            Select::File | Select::Edge | Select::Module | Select::Decision => Ok(Vec::new()),
        }
    }

    /// The symbols of `file` that are subjects of the decision's scope.
    fn check_symbols(&self, ctx: &Ctx, file: &File) -> Result<Vec<Diagnostic>, PluginError> {
        let project = ctx.project;
        let mut found = Vec::new();
        for symbol in project
            .symbols_in(&file.path)
            .filter(|s| ctx.applies.admits_symbol(project, s))
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
            let place = Place {
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
        ctx: &Ctx,
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
            let place = Place {
                file: file.path.clone(),
                span: comment.span,
                symbol: None,
            };
            found.extend(self.judge(value, place, &comment.text)?);
        }
        Ok(found)
    }

    fn check_project(&self, ctx: &Ctx) -> Result<Vec<Diagnostic>, PluginError> {
        let project = ctx.project;
        let first_file = module_anchors(project);
        let files_of = files_by_module(project);
        let mut found = Vec::new();
        match self.rule.select {
            Select::Module => {
                let stats = modules::stats(project);
                let none = modules::Stats::default();
                for module in &project.modules {
                    let Some((anchor, count)) = first_file.get(module.path.as_str()) else {
                        continue;
                    };
                    if !admits_module(ctx, &files_of, module) {
                        continue;
                    }
                    let files = files_of.get(module.path.as_str()).map_or(0, BTreeSet::len);
                    let value = cel_fact(&facts::module(
                        module,
                        files,
                        *count,
                        stats.get(module.path.as_str()).unwrap_or(&none),
                    ));
                    let place = Place {
                        file: anchor.file.clone(),
                        span: top_of_file(),
                        symbol: None,
                    };
                    found.extend(self.judge(value, place, &module.path)?);
                }
            }
            Select::Edge => {
                for edge in &project.edges {
                    let anchor = match &edge.from {
                        Node::Symbol(id) => project.symbol(id),
                        Node::Module(m) => first_file.get(m.as_str()).map(|(s, _)| *s),
                    };
                    let Some(anchor) = anchor else { continue };
                    let admitted = match &edge.from {
                        Node::Symbol(id) => project
                            .symbol(id)
                            .is_none_or(|s| ctx.applies.admits_symbol(project, s)),
                        Node::Module(m) => project
                            .module(m)
                            .is_none_or(|module| admits_module(ctx, &files_of, module)),
                    };
                    if !admitted {
                        continue;
                    }
                    let value = cel_fact(&facts::edge(project, edge));
                    let key = format!("{:?}->{:?}", edge.from, edge.to);
                    let place = Place {
                        file: anchor.file.clone(),
                        span: anchor.span,
                        symbol: None,
                    };
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
            let place = Place {
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
        let place = Place {
            file: symbol.file.clone(),
            span: symbol.span,
            symbol: Some(symbol.id.clone()),
        };
        self.judge(value, place, symbol.id.as_str())
    }

    /// Evaluates the rule for one selected value; a finding is reported at
    /// `place` unless the check says otherwise.
    fn judge(
        &self,
        value: Fact,
        place: Place,
        key: &str,
    ) -> Result<Option<Diagnostic>, PluginError> {
        let rule = self.rule;
        let mut frame = Frame::new(&self.meta.id, &rule.library);
        frame.set_fact(rule.select.variable(), value);
        frame.set_fact("options", self.options.clone_as_boxed());
        for (name, program) in &rule.bindings {
            let bound = frame.run(program)?;
            frame.set_cel(name, bound);
        }
        if !frame.truth(&rule.condition)? {
            return Ok(None);
        }
        let message = frame.text(&rule.message)?;
        let evidence = frame.evidence(&rule.evidence)?;
        let place = self.place(&frame, place)?;
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
        Ok(Some(diagnostic))
    }

    fn place(&self, frame: &Frame, default: Place) -> Result<Place, PluginError> {
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
            return Ok(Place {
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
        Ok(Place { span, ..default })
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

/// The files that declare a symbol of each module.
fn files_by_module(project: &Project) -> BTreeMap<&str, BTreeSet<&std::path::Path>> {
    let mut files: BTreeMap<&str, BTreeSet<&std::path::Path>> = BTreeMap::new();
    for symbol in &project.symbols {
        files
            .entry(symbol.id.module())
            .or_default()
            .insert(symbol.file.as_path());
    }
    files
}

/// Whether a module is a subject of the decision, by the files that declare it.
fn admits_module(
    ctx: &Ctx,
    files_of: &BTreeMap<&str, BTreeSet<&std::path::Path>>,
    module: &lighthouse_model::Module,
) -> bool {
    let files: Vec<&File> = files_of
        .get(module.path.as_str())
        .into_iter()
        .flatten()
        .filter_map(|path| ctx.project.file(path))
        .collect();
    ctx.applies.admits_module(module, &files)
}

/// The start of a file, where findings that belong to a whole file go.
fn top_of_file() -> Span {
    let p = Position { line: 1, col: 1 };
    Span { start: p, end: p }
}
