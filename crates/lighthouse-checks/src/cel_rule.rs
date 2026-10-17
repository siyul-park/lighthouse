//! A `cel` check: a `where` expression that is true for a violation of the
//! selected values, a message and evidence.

use std::{cell::RefCell, collections::BTreeMap};

use cel::Program;
use lighthouse_model::{Diagnostic, File, Node, Position, Project, Reach, Span, Symbol, SymbolId};
use lighthouse_plugin::{Context, Error as PluginError, RuleManifest};
use lighthouse_spec::{CelCheck, Decision, Select};
use serde_json::{Map, Value};

use crate::{
    Error,
    abstain::{Abstain, Candidate},
    builder::Builder,
    caching,
    eval::{Fact, Frame, Template, cel_fact, compile, compile_all},
    facts,
    library::{self, Needs},
    metrics::MEASURES,
    modules::{self, Declared},
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
    abstain: Option<Abstain>,
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
#[derive(Clone)]
struct Anchor {
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
        sources.extend(check.abstain.as_deref());
        let select = check
            .selects(decision.scope.subject)
            .ok_or_else(|| Error::invalid(id, "a project decision needs a `select`"))?;
        let maybe = |what: &str, source: &Option<String>| {
            source.as_deref().map(|s| compile(id, what, s)).transpose()
        };
        Ok(Self {
            select,
            abstain: Abstain::new(decision, check, select)?,
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

    /// Whether the check has a route that puts subjects to the models.
    pub(crate) fn reviews(&self) -> bool {
        self.abstain.is_some()
    }

    /// The analyzers whose facts the expressions read.
    pub(crate) fn analyzers(&self) -> Vec<String> {
        if self.needs.function("metrics") {
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
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let builder = Builder::new(ctx, &self.needs, options, &meta.id)?;
        let reviewing = self.reviews() && ctx.models.asks(&meta.id);
        let run = Evaluation {
            rule: self,
            reviewing,
            meta,
            builder,
            options: cel_fact(&Value::Object(options.clone())),
            candidates: RefCell::default(),
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
    options: Fact,
    /// The subjects `abstain` picked, to be put to the models.
    candidates: RefCell<Vec<Candidate>>,
}

impl Evaluation<'_> {
    fn check_file(&self, ctx: &Context) -> Result<Vec<Diagnostic>, PluginError> {
        let Some((file, text)) = ctx.file else {
            return Ok(Vec::new());
        };
        match self.rule.select {
            Select::File if ctx.applies.admits_file(file) => {
                let value = cel_fact(&self.builder.file(file, text));
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
                for edge in &project.edges {
                    let Some(anchor) = edge_anchor(ctx, &declared, &edge.from) else {
                        continue;
                    };
                    let value = cel_fact(&facts::edge(project, edge));
                    let key = format!("{:?}->{:?}", edge.from, edge.to);
                    let place = Anchor {
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
    /// `place` unless the check says otherwise.
    fn judge(
        &self,
        value: Fact,
        place: Anchor,
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

/// The symbol the findings about an edge are reported at, when the edge starts
/// at a subject of the decision's scope.
fn edge_anchor<'p>(
    ctx: &Context<'p>,
    declared: &BTreeMap<&str, Declared<'p>>,
    from: &Node,
) -> Option<&'p Symbol> {
    let project = ctx.project;
    match from {
        Node::Symbol(id) => project
            .symbol(id)
            .filter(|s| ctx.applies.admits_symbol(project, s)),
        Node::Module(m) => {
            let declared = declared.get(m.as_str())?;
            project
                .module(m)
                .is_none_or(|module| admits_module(ctx, declared, module))
                .then_some(declared.anchor)
        }
    }
}

/// Whether a module is a subject of the decision, by the files that declare it.
fn admits_module(ctx: &Context, declared: &Declared, module: &lighthouse_model::Module) -> bool {
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
