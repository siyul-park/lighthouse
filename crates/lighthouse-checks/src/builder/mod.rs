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
};

use lighthouse_model::{
    Position, Span,
    ucm::{Comment, FunctionSummary, LoadNeed, Model, Symbol, SymbolId, SymbolKind, Visibility},
};
use lighthouse_plugin::{Annotations, Context, Error, Reads, Workspace};
use serde_json::{Map, Value, json};

use crate::{
    fact::{FUNCTION, Fact, TEST},
    facts,
    indexes::Indexes,
    layout::{self, MODULE_BODY},
    library::Needs,
    naming::Tests,
};

mod api;
pub(crate) mod cohesion;
pub(crate) mod cycle;
pub(crate) mod effective;
mod file_facts;
mod file_uses;
mod forward;
mod gap;
pub(crate) mod homonyms;
pub(crate) mod imports;
pub(crate) mod owned;
mod ownership;
pub(crate) mod parent;
mod role;
pub(crate) mod split;
mod stability;
mod style;
mod unit;
mod widths;

use forward::{forwarded, reaches};

/// The fields of a node that are where it is and what it says.
const POSITIONAL: [&str; 6] = ["line", "col", "pos", "end_line", "documented", "doc"];

/// The summary of a function, read when a field asks for it: the summary moves
/// with the body of the function, so a node related to another one is built
/// without it unless an expression reads what it holds.
#[derive(Clone, Copy)]
pub(super) struct Summary<'p> {
    project: &'p dyn Model,
    symbol: &'p SymbolId,
}

impl<'p> Summary<'p> {
    pub(super) fn of(project: &'p dyn Model, symbol: &'p Symbol) -> Self {
        Self {
            project,
            symbol: &symbol.id,
        }
    }

    pub(super) fn get(self) -> Option<&'p FunctionSummary> {
        self.project.function(self.symbol)
    }
}

/// Builds the values of one check over one file (or the project).
pub(crate) struct Builder<'a> {
    project: &'a dyn Model,
    ws: &'a Workspace,
    needs: &'a Needs,
    language: String,
    naming: Tests<'a>,
    indexes: &'a dyn Indexes,
    annotations: Option<&'a Annotations>,
    declared: RefCell<BTreeMap<PathBuf, Rc<Vec<Vec<&'a Symbol>>>>>,
    related: RefCell<BTreeMap<SymbolId, Value>>,
    /// What each file needs of its own declarations as it loads, found once.
    loads: RefCell<BTreeMap<PathBuf, Rc<Vec<LoadNeed>>>>,
    related_needs: Needs,
}

/// What a builder reads: the code model, the project's indexes and the
/// workspace.
#[derive(Clone, Copy)]
pub struct Over<'a> {
    pub model: &'a dyn Model,
    pub indexes: &'a dyn Indexes,
    pub ws: &'a Workspace,
}

impl<'a> Builder<'a> {
    /// A builder of the facts the run of a check reads: what the analyzers
    /// measured and the directives the engine judged are the run's own.
    pub(crate) fn new(
        ctx: &'a Context<'a>,
        indexes: &'a dyn Indexes,
        needs: &'a Needs,
    ) -> Result<Self, Error> {
        let language = ctx
            .file
            .map_or_else(String::new, |(file, _)| file.analysis_language().to_owned());
        let over = Over {
            model: ctx.model,
            indexes,
            ws: ctx.workspace,
        };
        Ok(Self {
            annotations: Some(ctx.annotations),
            ..Self::over(over, needs, &language)
        })
    }

    /// A builder over a model, with no analyzer facts (`metrics` reads as not
    /// measured) and no directives: for a fix, and for the facts a graph
    /// holds.
    pub(crate) fn over(over: Over<'a>, needs: &'a Needs, language: &str) -> Self {
        let Over {
            model: project,
            indexes,
            ws,
        } = over;
        Self {
            project,
            ws,
            needs,
            language: language.to_owned(),
            naming: Tests::new(ws.test_naming(language), project, indexes),
            indexes,
            annotations: None,
            declared: RefCell::default(),
            related: RefCell::default(),
            loads: RefCell::default(),
            related_needs: needs.of_related(),
        }
    }

    /// What went wrong in reading the project's indexes, if anything.
    pub(crate) fn failure(&self) -> Option<Error> {
        self.indexes.failure()
    }

    pub(crate) fn project(&self) -> &'a dyn Model {
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
                    .filter(|symbol| {
                        facts::span_of(self.project, &symbol.id).start.line > end
                            && symbol.name != MODULE_BODY
                    })
                    .min_by_key(|symbol| {
                        (facts::span_of(self.project, &symbol.id).start, &symbol.id)
                    })
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
        let gap = next.map_or(0, |symbol| {
            blank_lines(
                text,
                end,
                facts::span_of(self.project, &symbol.id).start.line,
            )
        });
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
        self.enrich(&mut value, symbol, Summary::of(self.project, symbol));
        value
    }

    /// A symbol as a subject of `select: symbol`: its facts, and its function's
    /// as `func` when a check asks for them.
    pub(crate) fn symbol_value(&self, symbol: &Symbol) -> Value {
        let mut value = self.symbol(symbol);
        if self.needs.field("func") {
            value["func"] = self.function(symbol).unwrap_or(Value::Null);
        }
        value
    }

    /// A function or method with every fact its check asks for; `None` when
    /// the symbol is not a function (a type, a constant, a variable): it has
    /// no summary, is no subject of a function check, and its `func` is
    /// `null`. That is "not applicable", never a failure.
    pub(crate) fn function(&self, symbol: &Symbol) -> Option<Value> {
        self.project.function(&symbol.id)?;
        let mut value = facts::function(self.project, symbol)?;
        self.enrich(&mut value, symbol, Summary::of(self.project, symbol));
        Some(value)
    }

    /// A test with every fact its check asks for; `None` for a symbol that
    /// is no test.
    pub(crate) fn test(&self, symbol: &Symbol) -> Option<Value> {
        let case = self.project.test(&symbol.id)?;
        let mut value = facts::test(self.project, symbol, case);
        self.enrich(&mut value, symbol, Summary::of(self.project, symbol));
        Some(value)
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
                .symbols()
                .filter(|s| s.id.module() == module.path)
                .collect();
            symbols.sort_by_key(|s| (&s.file, facts::span_of(self.project, &s.id).start, &s.id));
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
        self.annotations?.of(file)
    }

    /// A file with the facts about its text and its symbols.
    pub(crate) fn file(&self, file: &lighthouse_model::ucm::File, text: &str) -> Value {
        let mut value = facts::file(self.project, file, text);
        // Whether a file is generated is what the merged model says, not what
        // the run read from disk.
        value["generated"] = json!(self.project.file(&file.path).is_none_or(|f| f.generated));
        if self.needs.field("symbols") {
            let mut symbols: Vec<&Symbol> = self.project.symbols_in(&file.path).collect();
            symbols.sort_by_key(|s| (facts::span_of(self.project, &s.id).start, &s.id));
            value["symbols"] = Value::Array(symbols.into_iter().map(|s| self.related(s)).collect());
        }
        if self.needs.field("comments") {
            value["comments"] = Value::Array(
                self.project
                    .comments_in(&file.path)
                    .map(|comment| self.comment(comment, text))
                    .collect(),
            );
        }
        self.file_facts(&mut value, file);
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

    fn enrich(&self, value: &mut Value, symbol: &Symbol, summary: Summary<'_>) {
        if let Some(map) = value.as_object_mut() {
            map.extend(self.extras(symbol, summary));
        }
    }

    /// The fields on top of the base fields of a symbol, which only the
    /// expressions of this check ask for.
    fn extras(&self, symbol: &Symbol, summary: Summary<'_>) -> Map<String, Value> {
        let mut extras = self.fields(symbol, summary, self.needs);
        self.relations(&mut extras, symbol);
        extras
    }

    /// The extra fields that are no relation to other nodes: what a node
    /// related to another one has, as the node itself.
    fn fields(&self, symbol: &Symbol, summary: Summary<'_>, needs: &Needs) -> Map<String, Value> {
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
            map.insert(
                "span".to_owned(),
                span(&symbol.file, facts::span_of(self.project, &symbol.id)),
            );
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
        summary: Summary<'_>,
        needs: &Needs,
    ) {
        let project = self.project;
        if needs.field("forwards_only") || needs.field("forward_target") {
            let target = summary.get().and_then(|_| forwarded(project, symbol));
            map.insert("forwards_only".to_owned(), json!(target.is_some()));
            map.insert(
                "forward_target".to_owned(),
                json!(target.map_or("", |t| t.as_str())),
            );
        }
        if needs.field("owned") && symbol.kind == SymbolKind::Test {
            map.insert("owned".to_owned(), self.owned(symbol));
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
        self.tie_facts(map, symbol, needs);
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
                json!(self.indexes.tested(symbol.id.module())),
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
        if needs.field("split") {
            map.insert("split".to_owned(), self.split(symbol));
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
        let summary = Summary::of(self.project, symbol);
        let needs = &self.related_needs;
        // Where a related node is moves with any edit above it, and what it says
        // with an edit of its comment: the rules that read neither of it do not
        // depend on either, and the node is built from its shape alone.
        let placed = POSITIONAL.iter().any(|name| needs.field(name));
        let mut value = facts::symbol_with(self.project, symbol, placed);
        if let Some(map) = value.as_object_mut() {
            map.retain(|name, _| !POSITIONAL.contains(&name.as_str()) || needs.field(name));
        }
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
        peers.sort_by_key(|s| (facts::span_of(self.project, &s.id).start, &s.id));
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
    /// matched and whether it is in a file the provider read from its source
    /// alone (`syntactic`).
    fn tests(&self, symbol: &Symbol) -> Value {
        Value::Array(
            self.naming
                .credited_tests(symbol)
                .into_iter()
                .filter_map(|(case, kind)| {
                    let test = self.project.symbol(&case.symbol)?;
                    let mut value = self.node(test);
                    value["match"] = json!(kind.as_str());
                    value["syntactic"] = json!(
                        self.project
                            .file(&test.file)
                            .is_some_and(lighthouse_model::ucm::File::is_syntactic)
                    );
                    Some(value)
                })
                .collect(),
        )
    }

    /// What is measured of a function, the numbers of its signature among them;
    /// `measured` says whether the analyzer measured it.
    fn metrics(&self, symbol: &Symbol, summary: Summary<'_>) -> Value {
        let summary = summary.get();
        let found = self.indexes.measured(symbol);
        let measures = found.unwrap_or_default();
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
        callers.sort_by_key(|s| (facts::span_of(self.project, &s.id).start, &s.id));
        Value::Array(callers.into_iter().map(|s| self.related(s)).collect())
    }

    fn file_module(&self, file: &lighthouse_model::ucm::File) -> Option<String> {
        self.project
            .symbols_in(&file.path)
            .min_by(|a, b| {
                (facts::span_of(self.project, &a.id).start, &a.id)
                    .cmp(&(facts::span_of(self.project, &b.id).start, &b.id))
            })
            .map(|s| s.id.module().to_owned())
    }

    /// The private symbols of the file's own module that the file's symbols
    /// reach, and the first symbol that reaches one.
    fn private_reach(&self, file: &lighthouse_model::ucm::File) -> Value {
        let project = self.project;
        let mut symbols: Vec<&Symbol> = project.symbols_in(&file.path).collect();
        symbols.sort_by_key(|s| (facts::span_of(self.project, &s.id).start, &s.id));
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

/// The facts of `symbol` as a subject of the sort `kind` (`symbol`,
/// `function` or `test`) for the fields `reads` asks for; `None` when it has
/// none of that sort. It is what `subjects` is held to.
#[cfg(test)]
pub fn subject(
    kind: &str,
    over: Over<'_>,
    reads: &Reads,
    symbol: &Symbol,
) -> Result<Option<Fact>, Error> {
    let needs = Needs::reading_fields(reads);
    let language = over
        .model
        .file(&symbol.file)
        .map_or("", |file| file.analysis_language());
    let builder = Builder::over(over, &needs, language);
    let found = subject_fact(&builder, kind, symbol);
    match builder.failure() {
        Some(error) => Err(error),
        None => Ok(found),
    }
}

/// The facts of the symbols `file` declares as subjects of the sort `kind`,
/// by id, for the fields `reads` asks for: what each of them is as `subject`
/// makes it, built with what the file shares.
pub fn subjects(
    kind: &str,
    over: Over<'_>,
    reads: &Reads,
    file: &Path,
) -> Result<Vec<(String, Option<Fact>)>, Error> {
    let needs = Needs::reading_fields(reads);
    let language = over
        .model
        .file(file)
        .map_or("", |file| file.analysis_language());
    let model = over.model;
    let builder = Builder::over(over, &needs, language);
    let found = model
        .symbols_in(file)
        .map(|symbol| {
            (
                symbol.id.as_str().to_owned(),
                subject_fact(&builder, kind, symbol),
            )
        })
        .collect();
    match builder.failure() {
        Some(error) => Err(error),
        None => Ok(found),
    }
}

/// The facts of `comment`, a comment of a file whose text is `text`.
pub fn comment_subject(over: Over<'_>, comment: &Comment, text: &str) -> Result<Fact, Error> {
    let needs = Needs::default();
    let language = over
        .model
        .file(&comment.file)
        .map_or("", |file| file.analysis_language());
    let builder = Builder::over(over, &needs, language);
    let value = builder.comment(comment, text);
    match builder.failure() {
        Some(error) => Err(error),
        None => Ok(Fact::from(&value)),
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

/// The modules that some test module tests with tests: a module is tested
/// when one of these is it or holds it.
pub fn tested(project: &dyn Model) -> BTreeSet<String> {
    project
        .modules()
        .filter(|m| project.tests_in(&m.path).next().is_some())
        .map(|m| m.test_of.clone().unwrap_or_else(|| m.path.clone()))
        .collect()
}

fn subject_fact(builder: &Builder<'_>, kind: &str, symbol: &Symbol) -> Option<Fact> {
    let value = match kind {
        FUNCTION => builder.function(symbol),
        TEST => builder.test(symbol),
        _ => Some(builder.symbol_value(symbol)),
    };
    value.map(|value| Fact::from(&value))
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
fn touched_by_tests(project: &dyn Model, symbol: &Symbol) -> bool {
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
fn data_only(project: &dyn Model, symbol: &Symbol) -> bool {
    symbol.kind == SymbolKind::Type
        && !project.members(&symbol.id).iter().any(|id| {
            project
                .symbol(id)
                .is_some_and(|m| m.kind == SymbolKind::Method)
        })
}
