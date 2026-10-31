//! The values a check's expressions see, with the facts the code model gives
//! them. The base fields of every value are cheap and always there; the facts
//! behind the library functions and the derived fields (`forwards_only`,
//! `receiver_affinity`, `local_callers`, ...) are computed once per run of a
//! check over a file, and only when an expression asks for them.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, OnceLock},
};

use crate::metrics::{MEASURES, Measures, propagation::System};
use lighthouse_model::{
    Position, Span,
    ucm::{Comment, FunctionSummary, Project, Symbol, SymbolId, SymbolKind, Visibility},
};
use lighthouse_plugin::{Annotations, Context, Error, Workspace};
use serde_json::{Map, Value, json};

use crate::{
    eval::{Fact, cel_fact, with_entries, with_fact},
    facts,
    layout::{self, MODULE_BODY, contains},
    library::Needs,
    naming::Tests,
    table::Table,
};

mod api;
mod cohesion;
mod cycle;
mod effective;
mod file_facts;
mod forward;
mod gap;
mod homonyms;
mod imports;
mod ownership;
mod parent;
mod role;
mod stability;
mod style;
mod unit;
mod widths;

use forward::{forwarded, reaches};

/// The indexes over the whole project that facts read, each built by the first
/// check of a run that asks and shared by the others.
#[derive(Default)]
pub(crate) struct Indexes {
    single: OnceLock<effective::Index>,
    imports: OnceLock<BTreeMap<String, Vec<String>>>,
    cycles: OnceLock<BTreeMap<String, Vec<String>>>,
    types: OnceLock<BTreeMap<String, Vec<Value>>>,
    names: OnceLock<BTreeMap<String, Vec<SymbolId>>>,
    parents: OnceLock<parent::Uses>,
    modules: OnceLock<System>,
    symbols: OnceLock<System>,
}

/// Builds the values of one run of a check over one file (or the project).
pub(crate) struct Builder<'a> {
    project: &'a Project,
    ws: &'a Workspace,
    needs: &'a Needs,
    language: String,
    naming: Tests<'a>,
    measures: Option<BTreeMap<SymbolId, Measures>>,
    table: Arc<Table>,
    indexes: Arc<Indexes>,
    annotations: Arc<Annotations>,
    declared: RefCell<BTreeMap<PathBuf, Rc<Vec<Vec<&'a Symbol>>>>>,
    related: RefCell<BTreeMap<SymbolId, Value>>,
    related_needs: Needs,
}

impl<'a> Builder<'a> {
    pub(crate) fn new(ctx: &'a Context<'a>, needs: &'a Needs) -> Result<Self, Error> {
        let measures = if needs.field("metrics") {
            Some(ctx.fact(MEASURES)?)
        } else {
            None
        };
        let language = ctx
            .file
            .map_or_else(String::new, |(file, _)| file.analysis_language().to_owned());
        Ok(Self {
            project: ctx.project,
            ws: ctx.workspace,
            needs,
            naming: Tests::new(
                ctx.workspace.test_naming(&language),
                ctx.project,
                ctx.memo.slot(),
            ),
            language,
            measures,
            table: ctx.memo.slot(),
            indexes: ctx.memo.slot(),
            annotations: ctx.memo.slot(),
            declared: RefCell::default(),
            related: RefCell::default(),
            related_needs: needs.of_related(),
        })
    }

    /// A builder outside a run of checks, for a fix: it has no analyzer facts
    /// (`metrics` reads as not measured) and shares no cache with other rules.
    pub(crate) fn standalone(
        project: &'a Project,
        ws: &'a Workspace,
        needs: &'a Needs,
        language: &str,
    ) -> Self {
        Self {
            project,
            ws,
            needs,
            language: language.to_owned(),
            naming: Tests::new(ws.test_naming(language), project, Arc::default()),
            measures: None,
            table: Arc::default(),
            indexes: Arc::default(),
            annotations: Arc::default(),
            declared: RefCell::default(),
            related: RefCell::default(),
            related_needs: needs.of_related(),
        }
    }

    pub(crate) fn project(&self) -> &'a Project {
        self.project
    }

    /// A node as a list holds it: the fields that identify a declaration.
    pub(crate) fn node(&self, symbol: &Symbol) -> Value {
        facts::node(self.project, symbol)
    }

    /// A comment of the focused file; `text` is the file's text.
    pub(crate) fn comment(&self, comment: &Comment, text: &str) -> Value {
        let line = text
            .lines()
            .nth(comment.span.start.line.saturating_sub(1) as usize)
            .unwrap_or("");
        let before = line
            .get(..comment.span.start.col.saturating_sub(1) as usize)
            .unwrap_or("")
            .trim();
        let end = comment.span.end.line;
        let next = comment
            .attached_to
            .as_ref()
            .and_then(|id| self.project.symbol(id))
            .filter(|symbol| symbol.name != MODULE_BODY)
            .or_else(|| {
                self.project
                    .symbols_in(&comment.file)
                    .filter(|symbol| symbol.span.start.line > end && symbol.name != MODULE_BODY)
                    .min_by_key(|symbol| (symbol.span.start, &symbol.id))
            });
        let bodies: Vec<&str> = comment
            .text
            .lines()
            .map(str::trim)
            .map(|line| line.trim_start_matches('/').trim())
            .filter(|line| !line.is_empty())
            .collect();
        let directive = bodies.first().is_some_and(|body| {
            [
                "go:",
                "+build",
                "nolint",
                "lint:",
                "lighthouse-",
                "#!",
                "Code generated",
            ]
            .iter()
            .any(|prefix| body.starts_with(prefix))
        });
        let banner = !bodies.is_empty()
            && bodies.iter().all(|body| {
                body.chars().all(|ch| "-=*~_+".contains(ch))
                    || ["MARK:", "#region", "#endregion"]
                        .iter()
                        .any(|prefix| body.starts_with(prefix))
            });
        let gap = next.map_or(0, |symbol| blank_lines(text, end, symbol.span.start.line));
        let blank = Span {
            start: Position {
                line: end + 1,
                col: 1,
            },
            end: Position {
                line: end + gap + 1,
                col: 1,
            },
        };
        json!({
            "file": comment.file.to_string_lossy(),
            "text": comment.text,
            "line": comment.span.start.line,
            "end_line": comment.span.end.line,
            "col": comment.span.start.col,
            "standalone": before.is_empty(),
            "before": before,
            "generated": self.project.file(&comment.file).is_none_or(|f| f.generated),
            "attached": comment.attached_to.is_some(),
            "directive": directive,
            "banner": banner,
            "top": comment.span.start.line == 1,
            "gap": gap,
            "blank": span(&comment.file, blank),
            "next": next.map_or("", |symbol| symbol.id.as_str()),
            "next_name": next.map_or("", |symbol| symbol.name.as_str()),
            "span": span(&comment.file, comment.span),
        })
    }

    /// A symbol with every fact its check asks for.
    pub(crate) fn symbol(&self, symbol: &Symbol) -> Value {
        let mut value = facts::symbol(self.project, symbol);
        self.enrich(&mut value, symbol, self.project.function(&symbol.id));
        value
    }

    /// Like [`Builder::symbol`], in the form the CEL runtime holds it; the
    /// base fields are built once per run for all checks.
    pub(crate) fn symbol_fact(&self, symbol: &Symbol) -> Fact {
        let extras = self.extras(symbol, self.project.function(&symbol.id));
        let base = || Some(cel_fact(&facts::symbol(self.project, symbol)));
        let fact = self
            .table
            .symbols
            .with(&symbol.id, base, |base| {
                with_entries(base.as_ref(), &extras)
            })
            .unwrap_or_else(|| cel_fact(&Value::Null));
        if !self.needs.field("func") {
            return fact;
        }
        let func = self
            .function_fact(symbol)
            .unwrap_or_else(|| cel_fact(&Value::Null));
        with_fact(fact, "func", func)
    }

    /// Like [`Builder::function`], in the form the CEL runtime holds it.
    /// `None` when the symbol is not a function (a type, a constant, a
    /// variable): it has no summary, is no subject of a function check, and its
    /// `func` is `null`. That is "not applicable", never a failure.
    pub(crate) fn function_fact(&self, symbol: &Symbol) -> Option<Fact> {
        let summary = self.project.function(&symbol.id)?;
        let extras = self.extras(symbol, Some(summary));
        let base = || facts::function(self.project, symbol).map(|v| cel_fact(&v));
        self.table.functions.with(&symbol.id, base, |base| {
            with_entries(base.as_ref(), &extras)
        })
    }

    /// Like [`Builder::test`], in the form the CEL runtime holds it.
    pub(crate) fn test_fact(&self, symbol: &Symbol) -> Option<Fact> {
        let case = self.project.test(&symbol.id)?;
        let extras = self.extras(symbol, self.project.function(&symbol.id));
        let base = || Some(cel_fact(&facts::test(self.project, symbol, case)));
        self.table.tests.with(&symbol.id, base, |base| {
            with_entries(base.as_ref(), &extras)
        })
    }

    /// A module with the facts its check asks for.
    pub(crate) fn module(
        &self,
        module: &lighthouse_model::ucm::Module,
        declared: &crate::modules::Declared,
    ) -> Value {
        let mut value = facts::module(module, declared);
        if self.needs.field("symbols") {
            let mut symbols: Vec<&Symbol> = self
                .project
                .symbols
                .iter()
                .filter(|s| s.id.module() == module.path)
                .collect();
            symbols.sort_by_key(|s| (&s.file, s.span.start, &s.id));
            value["symbols"] = Value::Array(symbols.into_iter().map(|s| self.related(s)).collect());
        }
        if self.needs.field("types") {
            value["types"] = self.module_types(&module.path);
        }
        if self.needs.field("cycle") {
            value["cycle"] = self.module_cycle(&module.path);
        }
        if self.needs.field("parent") {
            value["parent"] = self.module_parent(module);
        }
        if self.needs.field("stability") {
            value["stability"] = self.module_stability(module);
        }
        value
    }

    /// What the suppression directives of the file say, once the engine has
    /// judged them; `None` before.
    pub(crate) fn annotations(&self, file: &Path) -> Option<Vec<Value>> {
        self.annotations.of(file)
    }

    /// A file with the facts about its text and its symbols.
    pub(crate) fn file(&self, file: &lighthouse_model::ucm::File, text: &str) -> Value {
        let mut value = facts::file(self.project, file, text);
        // Whether a file is generated is what the merged model says, not what
        // the run read from disk.
        value["generated"] = json!(self.project.file(&file.path).is_none_or(|f| f.generated));
        if self.needs.field("symbols") {
            let mut symbols: Vec<&Symbol> = self.project.symbols_in(&file.path).collect();
            symbols.sort_by_key(|s| (s.span.start, &s.id));
            value["symbols"] = Value::Array(symbols.into_iter().map(|s| self.related(s)).collect());
        }
        if self.needs.field("comments") {
            value["comments"] = Value::Array(
                self.project
                    .comments_in(&file.path)
                    .iter()
                    .map(|comment| self.comment(comment, text))
                    .collect(),
            );
        }
        if self.needs.field("blocks") {
            value["blocks"] = self.blocks(file);
        }
        if self.needs.field("pair") {
            value["pair"] = self.pair(file);
        }
        if self.needs.field("line_widths") {
            value["line_widths"] = widths::line_widths(text);
        }
        if self.needs.field("annotations") {
            value["annotations"] = self
                .annotations(&file.path)
                .map_or(Value::Null, Value::Array);
        }
        if self.needs.field("private_reach") {
            value["private_reach"] = self.private_reach(file);
            value["module"] = json!(self.file_module(file).unwrap_or_default());
            value["module_known"] = json!(
                self.file_module(file)
                    .is_some_and(|m| self.project.module(&m).is_some())
            );
            value["module_test_of"] = json!(
                self.file_module(file)
                    .and_then(|m| self.project.module(&m))
                    .and_then(|m| m.test_of.clone())
                    .unwrap_or_default()
            );
        }
        value
    }

    fn enrich(&self, value: &mut Value, symbol: &Symbol, summary: Option<&FunctionSummary>) {
        if let Some(map) = value.as_object_mut() {
            map.extend(self.extras(symbol, summary));
        }
    }

    /// The fields on top of the base fields of a symbol, which only the
    /// expressions of this check ask for.
    fn extras(&self, symbol: &Symbol, summary: Option<&FunctionSummary>) -> Map<String, Value> {
        let mut extras = self.fields(symbol, summary, self.needs);
        self.relations(&mut extras, symbol);
        extras
    }

    /// The extra fields that are no relation to other nodes: what a node
    /// related to another one has, as the node itself.
    fn fields(
        &self,
        symbol: &Symbol,
        summary: Option<&FunctionSummary>,
        needs: &Needs,
    ) -> Map<String, Value> {
        let mut extras = Map::new();
        let map = &mut extras;
        if needs.field("metrics") {
            map.insert("metrics".to_owned(), self.metrics(symbol, summary));
        }
        if needs.field("unit") {
            map.insert("unit".to_owned(), self.unit(symbol, summary));
        }
        if needs.field("reach") {
            map.insert("reach".to_owned(), self.reach(symbol));
        }
        if needs.field("tests") {
            map.insert("tests".to_owned(), self.tests(symbol));
        }
        if needs.field("span") {
            map.insert("span".to_owned(), span(&symbol.file, symbol.span));
        }
        if needs.field("words") {
            map.insert("words".to_owned(), json!(crate::text::words(&symbol.name)));
        }
        self.facts(map, symbol, summary, needs);
        extras
    }

    fn facts(
        &self,
        map: &mut Map<String, Value>,
        symbol: &Symbol,
        summary: Option<&FunctionSummary>,
        needs: &Needs,
    ) {
        let project = self.project;
        if needs.field("forwards_only") || needs.field("forward_target") {
            let target = summary.and_then(|_| forwarded(project, symbol));
            map.insert("forwards_only".to_owned(), json!(target.is_some()));
            map.insert(
                "forward_target".to_owned(),
                json!(target.map_or("", |t| t.as_str())),
            );
        }
        if needs.field("receiver_affinity") {
            map.insert(
                "receiver_affinity".to_owned(),
                self.receiver_affinity(symbol),
            );
        }
        self.placement_facts(map, symbol);
        self.api_facts(map, symbol, summary);
        self.role_facts(map, symbol, summary);
        if needs.field("local_callers") {
            map.insert("local_callers".to_owned(), self.local_callers(symbol));
        }
        if needs.field("touched_by_tests") {
            map.insert(
                "touched_by_tests".to_owned(),
                json!(touched_by_tests(project, symbol)),
            );
        }
        if needs.field("data_only") {
            map.insert("data_only".to_owned(), json!(data_only(project, symbol)));
        }
        if needs.field("module_tested") {
            map.insert(
                "module_tested".to_owned(),
                json!(module_tested(project, symbol.id.module())),
            );
        }
        if needs.field("gap") {
            map.insert("gap".to_owned(), self.gap(symbol));
        }
    }

    /// The facts about where a symbol sits among the types, modules and tests
    /// of the project.
    fn placement_facts(&self, map: &mut Map<String, Value>, symbol: &Symbol) {
        let needs = self.needs;
        if needs.field("envy") {
            map.insert("envy".to_owned(), self.envy(symbol));
        }
        if needs.field("owner_home") {
            map.insert("owner_home".to_owned(), self.owner_home(symbol));
        }
        if needs.field("style_mix") {
            map.insert("style_mix".to_owned(), json!(self.style_mix(symbol)));
        }
        if needs.field("homonyms") {
            map.insert("homonyms".to_owned(), self.homonyms(symbol));
        }
        if needs.field("module_imports") {
            map.insert("module_imports".to_owned(), self.module_imports(symbol));
        }
    }

    /// The related nodes the expressions ask for: each one the same kind of
    /// value as the node it hangs from, with its own owner and members.
    fn relations(&self, map: &mut Map<String, Value>, symbol: &Symbol) {
        let project = self.project;
        let lists: [(&str, &[SymbolId]); 5] = [
            ("callers", project.callers(&symbol.id)),
            ("callees", project.callees(&symbol.id)),
            ("references", project.references(&symbol.id)),
            ("uses", project.uses(&symbol.id)),
            ("members", project.members(&symbol.id)),
        ];
        for (name, ids) in lists {
            if self.needs.field(name) {
                map.insert(name.to_owned(), self.nodes(ids));
            }
        }
        if self.needs.field("implementers") {
            let types = if symbol.kind == SymbolKind::Interface {
                project.implementers(&symbol.id)
            } else {
                &[]
            };
            map.insert("implementers".to_owned(), self.nodes(types));
        }
        if self.needs.field("owner") {
            map.insert("owner".to_owned(), self.owner(symbol));
        }
        if self.needs.field("prev") || self.needs.field("next") {
            let (prev, next) = self.siblings(symbol);
            map.insert("prev".to_owned(), prev);
            map.insert("next".to_owned(), next);
        }
    }

    /// Related nodes, in the order of the ids.
    fn nodes(&self, ids: &[SymbolId]) -> Value {
        Value::Array(
            ids.iter()
                .filter_map(|id| self.project.symbol(id))
                .map(|s| self.related(s))
                .collect(),
        )
    }

    /// A node as another node relates to it: the same value as the node, with
    /// every field the expressions read except its relations to other nodes,
    /// which stay one level deep. Its owner and members are the nodes of the
    /// base fields; reading any other relation is an error.
    fn related(&self, symbol: &Symbol) -> Value {
        if let Some(found) = self.related.borrow().get(&symbol.id) {
            return found.clone();
        }
        let value = self.build_related(symbol);
        self.related
            .borrow_mut()
            .insert(symbol.id.clone(), value.clone());
        value
    }

    /// A related node as the expressions read it; see [`Builder::related`].
    fn build_related(&self, symbol: &Symbol) -> Value {
        let summary = self.project.function(&symbol.id);
        let mut value = facts::symbol(self.project, symbol);
        let needs = &self.related_needs;
        let mut fields = self.fields(symbol, summary, needs);
        if needs.field("func") {
            let mut func = facts::function(self.project, symbol).unwrap_or(Value::Null);
            if let Some(map) = func.as_object_mut() {
                map.extend(fields.clone());
            }
            fields.insert("func".to_owned(), func);
        }
        if needs.field("owner") {
            fields.insert("owner".to_owned(), self.owner(symbol));
        }
        if needs.field("members") {
            let members = self.project.members(&symbol.id);
            fields.insert(
                "members".to_owned(),
                Value::Array(
                    members
                        .iter()
                        .filter_map(|id| self.project.symbol(id))
                        .map(|s| self.node(s))
                        .collect(),
                ),
            );
        }
        if let Some(map) = value.as_object_mut() {
            map.extend(fields);
        }
        value
    }

    /// The owner of a symbol as a node of the base fields, null for a
    /// declaration that has none.
    fn owner(&self, symbol: &Symbol) -> Value {
        symbol
            .owner
            .as_ref()
            .and_then(|o| self.project.symbol(o))
            .map_or(Value::Null, |o| self.node(o))
    }

    /// The declarations next to a symbol in its file, before and after it.
    fn siblings(&self, symbol: &Symbol) -> (Value, Value) {
        let project = self.project;
        let mut peers: Vec<&Symbol> = if layout::is_declaration(project, symbol) {
            self.declarations(&symbol.file)
                .iter()
                .find(|list| list.iter().any(|s| s.id == symbol.id))
                .cloned()
                .unwrap_or_default()
        } else {
            project
                .symbols_in(&symbol.file)
                .filter(|s| s.owner == symbol.owner)
                .collect()
        };
        peers.sort_by_key(|s| (s.span.start, &s.id));
        let Some(at) = peers.iter().position(|s| s.id == symbol.id) else {
            return (Value::Null, Value::Null);
        };
        let node = |s: Option<&&Symbol>| s.map_or(Value::Null, |s| self.related(s));
        (
            node(at.checked_sub(1).and_then(|i| peers.get(i))),
            node(peers.get(at + 1)),
        )
    }

    /// The declarations of a file, grouped as they sit, found once per file.
    fn declarations(&self, path: &Path) -> Rc<Vec<Vec<&'a Symbol>>> {
        let mut declared = self.declared.borrow_mut();
        if let Some(found) = declared.get(path) {
            return Rc::clone(found);
        }
        let found = Rc::new(
            self.project
                .file(path)
                .map(|file| layout::declarations_in(self.project, file))
                .unwrap_or_default(),
        );
        declared.insert(path.to_owned(), Rc::clone(&found));
        found
    }

    /// The credited tests of a symbol, each a node with the way its name
    /// matched.
    fn tests(&self, symbol: &Symbol) -> Value {
        Value::Array(
            self.naming
                .credited_tests(symbol)
                .into_iter()
                .filter_map(|(case, kind)| {
                    let mut value = self.node(self.project.symbol(&case.symbol)?);
                    value["match"] = json!(kind.as_str());
                    Some(value)
                })
                .collect(),
        )
    }

    /// What is measured of a function, the numbers of its signature among them;
    /// `measured` says whether the analyzer measured it.
    fn metrics(&self, symbol: &Symbol, summary: Option<&FunctionSummary>) -> Value {
        let found = self.measures.as_ref().and_then(|m| m.get(&symbol.id));
        let measures = found.copied().unwrap_or_default();
        json!({
            "lines": measures.lines,
            "statements": measures.statements,
            "cyclomatic": measures.cyclomatic,
            "cognitive": measures.cognitive,
            "depth": measures.nesting,
            "params": summary.map_or(0, |s| s.params),
            "results": summary.map_or(0, |s| s.returns),
            "fan_in": measures.fan_in,
            "fan_out": measures.fan_out,
            "dispatcher": measures.dispatcher,
            "flat_dispatch": measures.flat_dispatch,
            "measured": found.is_some(),
        })
    }

    /// The production callers of a private function in source order, when every
    /// one of them lives in the function's file, it is never used as a value,
    /// and no call leads back to it. Empty when the order cannot be judged
    /// with confidence.
    fn local_callers(&self, callee: &Symbol) -> Value {
        let project = self.project;
        let entry = matches!(callee.name.as_str(), "main" | "init");
        if callee.visibility != Visibility::Private
            || entry
            || !project.references(&callee.id).is_empty()
            || callee.kind == SymbolKind::Test
        {
            return json!([]);
        }
        let mut callers: Vec<&Symbol> = project
            .callers(&callee.id)
            .iter()
            .filter(|id| !project.in_test(id))
            .filter_map(|id| project.symbol(id))
            .collect();
        // A package variable that calls it is a table of handlers: the order
        // of its entries says nothing about who the helper belongs to. The
        // statements a module runs when it loads call it where it has to be
        // defined already.
        if callers.is_empty()
            || callers
                .iter()
                .any(|s| s.kind == SymbolKind::Var || s.name == MODULE_BODY)
            || !callers.iter().all(|s| s.file == callee.file)
            || reaches(
                project,
                &callee.id,
                &callers.iter().map(|s| &s.id).collect::<Vec<_>>(),
            )
        {
            return json!([]);
        }
        callers.sort_by_key(|s| (s.span.start, &s.id));
        Value::Array(callers.into_iter().map(|s| self.related(s)).collect())
    }

    fn file_module(&self, file: &lighthouse_model::ucm::File) -> Option<String> {
        self.project
            .symbols_in(&file.path)
            .min_by(|a, b| (a.span.start, &a.id).cmp(&(b.span.start, &b.id)))
            .map(|s| s.id.module().to_owned())
    }

    /// The private symbols of the file's own module that the file's symbols
    /// reach, and the first symbol that reaches one.
    fn private_reach(&self, file: &lighthouse_model::ucm::File) -> Value {
        let project = self.project;
        let mut symbols: Vec<&Symbol> = project.symbols_in(&file.path).collect();
        symbols.sort_by_key(|s| (s.span.start, &s.id));
        let Some(first) = symbols.first() else {
            return json!({ "symbols": [], "first": "" });
        };
        let module = first.id.module();
        let mut reached: BTreeSet<&SymbolId> = BTreeSet::new();
        let mut at = None;
        for symbol in &symbols {
            let private: Vec<&SymbolId> = project
                .uses(&symbol.id)
                .iter()
                .filter(|id| id.module() == module)
                .filter(|id| !project.in_test(id))
                .filter(|id| {
                    project
                        .symbol(id)
                        .is_some_and(|s| s.visibility == Visibility::Private)
                })
                .collect();
            if !private.is_empty() {
                at.get_or_insert(*symbol);
                reached.extend(private);
            }
        }
        json!({
            "symbols": reached.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
            "first": at.map_or("", |s| s.id.as_str()),
        })
    }
}

/// A span as an expression sees it: where it is, in which file.
pub(crate) fn span(file: &Path, span: Span) -> Value {
    json!({
        "file": file.to_string_lossy(),
        "start": { "line": span.start.line, "col": span.start.col },
        "end": { "line": span.end.line, "col": span.end.col },
    })
}

/// The number of blank lines between line `after` and line `before` (1-based,
/// both outside the count), and zero when anything else sits between them.
fn blank_lines(text: &str, after: u32, before: u32) -> u32 {
    let count = before.saturating_sub(after).saturating_sub(1);
    let blank = text
        .lines()
        .skip(after as usize)
        .take(count as usize)
        .all(|line| line.trim().is_empty());
    if blank { count } else { 0 }
}

/// Whether test code uses the symbol, or a member of it. Heuristic references
/// count: a call through a receiver of unknown type may reach the symbol.
fn touched_by_tests(project: &Project, symbol: &Symbol) -> bool {
    let used_by_tests = |id| {
        project
            .callers(id)
            .iter()
            .chain(project.references(id))
            .any(|user| project.in_test(user))
    };
    used_by_tests(&symbol.id) || project.members(&symbol.id).iter().any(used_by_tests)
}

/// A type that declares no method: it is specified by the code that builds and
/// reads it.
fn data_only(project: &Project, symbol: &Symbol) -> bool {
    symbol.kind == SymbolKind::Type
        && !project.members(&symbol.id).iter().any(|id| {
            project
                .symbol(id)
                .is_some_and(|m| m.kind == SymbolKind::Method)
        })
}

/// Whether some test module tests `module` or an ancestor of it, with tests.
fn module_tested(project: &Project, module: &str) -> bool {
    project.modules.iter().any(|m| {
        let tested = m.test_of.as_deref().unwrap_or(&m.path);
        contains(tested, module) && !project.tests_in(&m.path).is_empty()
    })
}
