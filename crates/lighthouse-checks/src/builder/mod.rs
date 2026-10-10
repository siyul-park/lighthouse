//! The values a check's expressions see, with the facts the code model gives
//! them. The base fields of every value are cheap and always there; the facts
//! behind the library functions and the derived fields (`forwards_only`,
//! `receiver_affinity`, `local_callers`, ...) are computed once per run of a
//! check over a file, and only when an expression asks for them.

use std::{
    cell::OnceCell,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::metrics::{
    COGNITIVE, CYCLOMATIC, FAN, Fan, NESTING, SIZE, Size, is_dispatcher, is_flat_dispatch, read,
};
use lighthouse_model::{
    Comment, Edge, FunctionSummary, Node, Project, Symbol, SymbolId, SymbolKind, Target,
    Visibility, annotation,
};
use lighthouse_plugin::{Ctx, Error, KeyCtx, OrderKey, Workspace};
use serde_json::{Map, Value, json};

use crate::{
    eval::{Fact, cel_fact, with_entries},
    facts,
    library::Needs,
    naming::{Naming, Tests},
    table::Table,
};

mod effective;
mod hidden;
mod homonyms;
mod ownership;
mod role;

use role::satisfies_interface;

/// The analyzers a check needs when its expressions call `metrics`.
pub(crate) const METRIC_ANALYZERS: [&str; 5] = [SIZE, CYCLOMATIC, COGNITIVE, NESTING, FAN];

/// What was measured for the functions of one file.
#[derive(Default)]
struct Measures {
    sizes: BTreeMap<SymbolId, Size>,
    cyclomatic: BTreeMap<SymbolId, u32>,
    cognitive: BTreeMap<SymbolId, u32>,
    nesting: BTreeMap<SymbolId, u32>,
    fans: BTreeMap<SymbolId, Fan>,
}

/// Builds the values of one run of a check over one file (or the project).
pub(crate) struct Builder<'a> {
    project: &'a Project,
    ws: &'a Workspace,
    needs: &'a Needs,
    rule: &'a str,
    language: String,
    naming: Option<Tests<'a>>,
    keys: Vec<(String, &'a dyn OrderKey)>,
    measures: Option<Measures>,
    table: Arc<Table>,
    names: Arc<homonyms::Names>,
    single: Arc<effective::Single>,
    allow_suffixes: Vec<String>,
    first_test: OnceCell<Option<Value>>,
}

impl<'a> Builder<'a> {
    pub(crate) fn new(
        ctx: &'a Ctx<'a>,
        needs: &'a Needs,
        options: &'a Map<String, Value>,
        rule: &'a str,
    ) -> Result<Self, Error> {
        let measures = if needs.function("metrics") {
            Some(measures(ctx)?)
        } else {
            None
        };
        let keys = needs
            .keys()
            .into_iter()
            .filter_map(|id| Some((id.clone(), ctx.keys.key(&id)?)))
            .collect();
        Ok(Self {
            project: ctx.project,
            ws: ctx.ws,
            needs,
            rule,
            language: ctx
                .file
                .map_or_else(String::new, |(file, _)| file.lang.clone()),
            naming: Naming::from_options(options)
                .map(|naming| Tests::new(naming, ctx.project, ctx.memo.slot())),
            keys,
            measures,
            table: ctx.memo.slot(),
            names: ctx.memo.slot(),
            single: ctx.memo.slot(),
            allow_suffixes: strings(options.get("allowSuffixes")),
            first_test: OnceCell::new(),
        })
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
        let before = line.get(..comment.span.start.col.saturating_sub(1) as usize);
        json!({
            "file": comment.file.to_string_lossy(),
            "text": comment.text,
            "line": comment.span.start.line,
            "end_line": comment.span.end.line,
            "col": comment.span.start.col,
            "standalone": before.is_none_or(|b| b.trim().is_empty()),
            "generated": self.project.file(&comment.file).is_none_or(|f| f.generated),
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
        self.table
            .symbols
            .with(&symbol.id, base, |base| {
                with_entries(base.as_ref(), &extras)
            })
            .unwrap_or_else(|| cel_fact(&Value::Null))
    }

    /// Like [`Builder::function`], in the form the CEL runtime holds it.
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

    /// A file with the facts about its text and its symbols.
    pub(crate) fn file(&self, file: &lighthouse_model::File, text: &str) -> Value {
        let mut value = facts::file(self.project, file, text);
        // Whether a file is generated is what the merged model says, not what
        // the run read from disk.
        value["generated"] = json!(self.project.file(&file.path).is_none_or(|f| f.generated));
        if self.needs.mentions("private_reach") {
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
        let mut extras = Map::new();
        let map = &mut extras;
        let project = self.project;
        let needs = self.needs;
        if needs.function("metrics") {
            map.insert("__metrics".to_owned(), self.metrics(symbol, summary));
        }
        if needs.function("callers") {
            map.insert(
                "__callers".to_owned(),
                self.nodes(project.callers(&symbol.id)),
            );
        }
        if needs.function("callees") {
            map.insert(
                "__callees".to_owned(),
                self.nodes(project.callees(&symbol.id)),
            );
        }
        if needs.function("owner") {
            map.insert("__owner".to_owned(), self.owner(symbol));
        }
        if needs.function("edges") {
            map.insert("__edges".to_owned(), self.edges(symbol));
        }
        if needs.function("annotations") {
            map.insert("__annotations".to_owned(), self.annotations(symbol));
        }
        if needs.function("tests") {
            map.insert("__tests".to_owned(), self.tests(symbol));
        }
        if !self.keys.is_empty() {
            map.insert("__ranks".to_owned(), self.ranks(symbol));
        }
        self.facts(map, symbol, summary);
        extras
    }

    fn facts(
        &self,
        map: &mut Map<String, Value>,
        symbol: &Symbol,
        summary: Option<&FunctionSummary>,
    ) {
        let needs = self.needs;
        let project = self.project;
        if needs.mentions("forwards_only") || needs.mentions("forward_target") {
            let target = summary.and_then(|_| forwarded(project, symbol));
            map.insert("forwards_only".to_owned(), json!(target.is_some()));
            map.insert(
                "forward_target".to_owned(),
                json!(target.map_or("", |t| t.as_str())),
            );
        }
        if needs.mentions("satisfies_interface") {
            map.insert(
                "satisfies_interface".to_owned(),
                json!(satisfies_interface(project, symbol)),
            );
        }
        if needs.mentions("documented_by_interface") {
            map.insert(
                "documented_by_interface".to_owned(),
                json!(documented_by_interface(project, symbol)),
            );
        }
        if needs.mentions("receiver_affinity") {
            map.insert(
                "receiver_affinity".to_owned(),
                self.receiver_affinity(symbol),
            );
        }
        self.placement_facts(map, symbol);
        self.role_facts(map, symbol, summary);
        if needs.mentions("local_callers") {
            map.insert("local_callers".to_owned(), self.local_callers(symbol));
        }
        if needs.mentions("touched_by_tests") {
            map.insert(
                "touched_by_tests".to_owned(),
                json!(touched_by_tests(project, symbol)),
            );
        }
        if needs.mentions("data_only") {
            map.insert("data_only".to_owned(), json!(data_only(project, symbol)));
        }
        if needs.mentions("module_tested") {
            map.insert(
                "module_tested".to_owned(),
                json!(module_tested(project, symbol.id.module())),
            );
        }
        if needs.mentions("file_first_test") {
            map.insert(
                "file_first_test".to_owned(),
                self.first_test(symbol).unwrap_or_else(|| json!({})),
            );
        }
        if needs.mentions("helper_user") {
            map.insert("helper_user".to_owned(), self.helper_user(symbol));
        }
    }

    /// The facts about where a symbol sits among the types, modules and tests
    /// of the project.
    fn placement_facts(&self, map: &mut Map<String, Value>, symbol: &Symbol) {
        let needs = self.needs;
        if needs.mentions("envy") {
            map.insert("envy".to_owned(), self.envy(symbol));
        }
        if needs.mentions("owner_home") {
            map.insert("owner_home".to_owned(), self.owner_home(symbol));
        }
        if needs.mentions("homonyms") {
            map.insert("homonyms".to_owned(), self.homonyms(symbol));
        }
        if needs.mentions("hidden_target") {
            map.insert("hidden_target".to_owned(), self.hidden_target(symbol));
        }
    }

    fn nodes(&self, ids: &[SymbolId]) -> Value {
        Value::Array(
            ids.iter()
                .filter_map(|id| self.project.symbol(id))
                .map(|s| self.node(s))
                .collect(),
        )
    }

    fn owner(&self, symbol: &Symbol) -> Value {
        symbol
            .owner
            .as_ref()
            .and_then(|o| self.project.symbol(o))
            .map_or_else(|| json!({ "visibility": "", "kind": "" }), |o| self.node(o))
    }

    fn edges(&self, symbol: &Symbol) -> Value {
        let mut by_kind: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
        for edge in self.project.edges_from(&symbol.id) {
            by_kind
                .entry(edge.kind.as_str())
                .or_default()
                .push(self.edge_target(edge));
        }
        json!(by_kind)
    }

    fn edge_target(&self, edge: &Edge) -> Value {
        let resolution = serde_json::to_value(edge.resolution).unwrap_or(Value::Null);
        match &edge.to {
            Target::Resolved(Node::Symbol(id)) => match self.project.symbol(id) {
                Some(s) => {
                    let mut node = self.node(s);
                    node["resolution"] = resolution;
                    node
                }
                None => json!({ "id": id.as_str(), "resolution": resolution }),
            },
            Target::Resolved(Node::Module(m)) | Target::Path(m) => {
                json!({ "id": m, "module": m, "name": m, "kind": "module", "resolution": resolution })
            }
        }
    }

    fn annotations(&self, symbol: &Symbol) -> Value {
        Value::Array(
            self.project
                .comments_in(&symbol.file)
                .iter()
                .filter(|c| c.attached_to.as_ref() == Some(&symbol.id))
                .flat_map(|c| {
                    annotation::directives(&c.text)
                        .into_iter()
                        .filter(|(_, d)| d.form.needs_reason())
                        .map(|(_, d)| {
                            json!({
                                "form": d.form.name(),
                                "rules": d.rules,
                                "reason": d.reason.unwrap_or_default(),
                                "line": c.span.start.line,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .collect(),
        )
    }

    fn tests(&self, symbol: &Symbol) -> Value {
        let Some(naming) = &self.naming else {
            return json!([]);
        };
        Value::Array(
            naming
                .credited_tests(symbol)
                .into_iter()
                .map(|(case, kind)| {
                    json!({
                        "symbol": case.symbol.as_str(),
                        "name": case.symbol.as_str().rsplit_once('#').map_or("", |(head, _)| head.rsplit("::").next().unwrap_or("")),
                        "match": kind.as_str(),
                    })
                })
                .collect(),
        )
    }

    fn ranks(&self, symbol: &Symbol) -> Value {
        let empty = Map::new();
        let language = self
            .project
            .file(&symbol.file)
            .map_or(self.language.as_str(), |f| f.lang.as_str());
        let ctx = KeyCtx {
            project: self.project,
            language,
            rule: self.rule,
            options: &empty,
            constructors: self.ws.constructor_prefixes(language),
        };
        let ranks: BTreeMap<&str, i64> = self
            .keys
            .iter()
            .map(|(id, key)| {
                let rank = key
                    .rank(&ctx, symbol)
                    .ok()
                    .flatten()
                    .and_then(|r| i64::try_from(r).ok())
                    .unwrap_or(-1);
                (id.as_str(), rank)
            })
            .collect();
        json!(ranks)
    }

    fn metrics(&self, symbol: &Symbol, summary: Option<&FunctionSummary>) -> Value {
        let measures = self.measures.as_ref();
        let found = measures.and_then(|m| {
            Some((
                m.sizes.get(&symbol.id)?,
                m.cyclomatic.get(&symbol.id)?,
                m.cognitive.get(&symbol.id)?,
                m.nesting.get(&symbol.id)?,
                m.fans.get(&symbol.id)?,
            ))
        });
        let Some((size, cyclomatic, cognitive, nesting, fan)) = found else {
            return json!({ "measured": false, "cyclomatic": 0, "cognitive": 0, "statements": 0, "lines": 0, "nesting": 0, "fan_in": 0, "fan_out": 0, "dispatcher": false, "flat_dispatch": false });
        };
        json!({
            "measured": true,
            "cyclomatic": cyclomatic,
            "cognitive": cognitive,
            "statements": size.statements,
            "lines": size.lines,
            "nesting": nesting,
            "fan_in": fan.fan_in,
            "fan_out": fan.fan_out,
            "dispatcher": summary.is_some_and(is_dispatcher),
            "flat_dispatch": summary.is_some_and(is_flat_dispatch),
        })
    }

    /// The first test of the symbol's file, by position.
    fn first_test(&self, symbol: &Symbol) -> Option<Value> {
        self.first_test
            .get_or_init(|| {
                self.project
                    .symbols_in(&symbol.file)
                    .filter(|s| s.kind == SymbolKind::Test)
                    .min_by(|a, b| (a.span.start, &a.id).cmp(&(b.span.start, &b.id)))
                    .map(|s| self.node(s))
            })
            .clone()
    }

    /// The first symbol of the same file, after the helper, that calls or
    /// references it.
    fn helper_user(&self, symbol: &Symbol) -> Value {
        let project = self.project;
        project
            .callers(&symbol.id)
            .iter()
            .chain(project.references(&symbol.id))
            .filter_map(|id| project.symbol(id))
            .filter(|user| user.file == symbol.file && user.span.start > symbol.span.start)
            .min_by_key(|user| user.span.start)
            .map_or_else(|| json!({}), |user| self.node(user))
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
        // of its entries says nothing about who the helper belongs to.
        if callers.is_empty()
            || callers.iter().any(|s| s.kind == SymbolKind::Var)
            || !callers.iter().all(|s| s.file == callee.file)
            || cyclic(project, callee, &callers)
        {
            return json!([]);
        }
        callers.sort_by_key(|s| (s.span.start, &s.id));
        Value::Array(callers.into_iter().map(|s| self.ranked(s)).collect())
    }

    /// A node with the ranks of the order keys the expressions name.
    fn ranked(&self, symbol: &Symbol) -> Value {
        let mut value = self.node(symbol);
        if !self.keys.is_empty()
            && let Some(map) = value.as_object_mut()
        {
            map.insert("__ranks".to_owned(), self.ranks(symbol));
        }
        value
    }

    fn file_module(&self, file: &lighthouse_model::File) -> Option<String> {
        self.project
            .symbols_in(&file.path)
            .min_by(|a, b| (a.span.start, &a.id).cmp(&(b.span.start, &b.id)))
            .map(|s| s.id.module().to_owned())
    }

    /// The private symbols of the file's own module that the file's symbols
    /// reach, and the first symbol that reaches one.
    fn private_reach(&self, file: &lighthouse_model::File) -> Value {
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

/// Whether a method implements a documented method of an interface its type
/// implements: it is documented where the interface declares it.
fn documented_by_interface(project: &Project, method: &Symbol) -> bool {
    let Some(owner) = method
        .owner
        .as_ref()
        .filter(|_| method.kind == SymbolKind::Method)
    else {
        return false;
    };
    project.implements(owner).iter().any(|interface| {
        project.members(interface).iter().any(|id| {
            project.symbol(id).is_some_and(|declared| {
                declared.kind == SymbolKind::Method
                    && declared.name == method.name
                    && declared.doc.is_some()
            })
        })
    })
}

/// The strings of an option that is a list of them; empty when it is not.
fn strings(option: Option<&Value>) -> Vec<String> {
    option
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

fn measures(ctx: &Ctx) -> Result<Measures, Error> {
    Ok(Measures {
        sizes: read(ctx, SIZE)?,
        cyclomatic: read(ctx, CYCLOMATIC)?,
        cognitive: read(ctx, COGNITIVE)?,
        nesting: read(ctx, NESTING)?,
        fans: read(ctx, FAN)?,
    })
}

/// The private target a private undocumented wrapper only forwards to. The
/// wrapper must have exactly one caller, test callers included, and no use as
/// a value; a method named like an interface method may be reached through
/// that interface, so it is left alone.
fn forwarded<'p>(project: &'p Project, wrapper: &Symbol) -> Option<&'p SymbolId> {
    if wrapper.visibility != Visibility::Private
        || wrapper.doc.is_some()
        || project.callers(&wrapper.id).len() != 1
        || !project.references(&wrapper.id).is_empty()
        || satisfies_interface(project, wrapper)
    {
        return None;
    }
    let summary = project.function(&wrapper.id)?;
    let Some(Target::Resolved(Node::Symbol(target))) = &summary.forwards_to else {
        return None;
    };
    let callee = project.symbol(target)?;
    let eligible = callee.visibility == Visibility::Private
        && target.module() == wrapper.id.module()
        && callee.kind == wrapper.kind
        && project.callers(target).len() == 1
        && !reaches(project, target, &wrapper.id);
    eligible.then_some(target)
}

fn reaches(project: &Project, from: &SymbolId, goal: &SymbolId) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from];
    while let Some(current) = stack.pop() {
        if current == goal {
            return true;
        }
        if seen.insert(current) {
            stack.extend(project.callees(current));
        }
    }
    false
}

/// Whether the callee reaches one of its callers: recursion through others.
fn cyclic(project: &Project, callee: &Symbol, callers: &[&Symbol]) -> bool {
    let goals: BTreeSet<&SymbolId> = callers.iter().map(|s| &s.id).collect();
    let mut seen = BTreeSet::new();
    let mut stack: Vec<&SymbolId> = project.callees(&callee.id).iter().collect();
    while let Some(current) = stack.pop() {
        if goals.contains(current) {
            return true;
        }
        if seen.insert(current) {
            stack.extend(project.callees(current));
        }
    }
    false
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
        let covers = tested == module
            || module
                .strip_prefix(tested)
                .is_some_and(|rest| rest.starts_with('/'));
        covers && !project.tests_in(&m.path).is_empty()
    })
}
