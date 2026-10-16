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

use crate::metrics::{MEASURES, Measures};
use lighthouse_model::{
    Comment, FunctionSummary, Project, Symbol, SymbolId, SymbolKind, Visibility,
};
use lighthouse_plugin::{Context, Error, KeyContext, OrderKey, Workspace};
use serde_json::{Map, Value, json};

use crate::{
    eval::{Fact, cel_fact, with_entries, with_fact},
    facts,
    library::Needs,
    naming::{Naming, Tests},
    table::Table,
};

mod api;
mod cohesion;
mod effective;
mod forward;
mod gap;
mod imports;
mod ownership;
mod parent;
mod role;
mod stability;
mod unit;
mod widths;

use forward::{cyclic, forwarded};

/// Builds the values of one run of a check over one file (or the project).
pub(crate) struct Builder<'a> {
    project: &'a Project,
    ws: &'a Workspace,
    needs: &'a Needs,
    rule: &'a str,
    language: String,
    naming: Option<Tests<'a>>,
    keys: Vec<(String, &'a dyn OrderKey)>,
    measures: Option<BTreeMap<SymbolId, Measures>>,
    table: Arc<Table>,
    single: Arc<effective::Single>,
    imports: Arc<imports::Imports>,
    types: Arc<cohesion::Types>,
    parents: Arc<parent::Parents>,
    stabilities: Arc<stability::Stabilities>,
    allow_suffixes: Vec<String>,
    first_test: OnceCell<Option<Value>>,
}

impl<'a> Builder<'a> {
    pub(crate) fn new(
        ctx: &'a Context<'a>,
        needs: &'a Needs,
        options: &'a Map<String, Value>,
        rule: &'a str,
    ) -> Result<Self, Error> {
        let measures = if needs.function("metrics") {
            Some(ctx.fact(MEASURES)?)
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
            ws: ctx.workspace,
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
            single: ctx.memo.slot(),
            imports: ctx.memo.slot(),
            types: ctx.memo.slot(),
            parents: ctx.memo.slot(),
            stabilities: ctx.memo.slot(),
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
        let before = line
            .get(..comment.span.start.col.saturating_sub(1) as usize)
            .unwrap_or("")
            .trim();
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
        if !self.needs.fact("func") {
            return fact;
        }
        let func = self
            .function_fact(symbol)
            .unwrap_or_else(|| cel_fact(&Value::Null));
        with_fact(fact, "func", func)
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

    /// A module with the facts its check asks for.
    pub(crate) fn module(
        &self,
        module: &lighthouse_model::Module,
        files: usize,
        symbols: usize,
        stats: &crate::modules::Stats,
    ) -> Value {
        let mut value = facts::module(module, files, symbols, stats);
        if self.needs.fact("types") {
            value["types"] = self.module_types(&module.path);
        }
        if self.needs.fact("parent") {
            value["parent"] = self.module_parent(module);
        }
        if self.needs.fact("stability") {
            value["stability"] = self.module_stability(module);
        }
        value
    }

    /// A file with the facts about its text and its symbols.
    pub(crate) fn file(&self, file: &lighthouse_model::File, text: &str) -> Value {
        let mut value = facts::file(self.project, file, text);
        // Whether a file is generated is what the merged model says, not what
        // the run read from disk.
        value["generated"] = json!(self.project.file(&file.path).is_none_or(|f| f.generated));
        if self.needs.fact("line_widths") {
            value["line_widths"] = widths::line_widths(text);
        }
        if self.needs.fact("private_reach") {
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
            map.insert("__metrics".to_owned(), self.metrics(symbol));
        }
        if needs.function("callers") {
            map.insert(
                "__callers".to_owned(),
                self.nodes(project.callers(&symbol.id)),
            );
        }
        if needs.function("unit") {
            map.insert("__unit".to_owned(), self.unit(symbol, summary));
        }
        if needs.function("reach") {
            map.insert("__reach".to_owned(), self.reach(symbol));
        }
        if needs.function("owner") {
            map.insert("__owner".to_owned(), self.owner(symbol));
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
        if needs.fact("forwards_only") || needs.fact("forward_target") {
            let target = summary.and_then(|_| forwarded(project, symbol));
            map.insert("forwards_only".to_owned(), json!(target.is_some()));
            map.insert(
                "forward_target".to_owned(),
                json!(target.map_or("", |t| t.as_str())),
            );
        }
        if needs.fact("receiver_affinity") {
            map.insert(
                "receiver_affinity".to_owned(),
                self.receiver_affinity(symbol),
            );
        }
        self.placement_facts(map, symbol);
        self.api_facts(map, symbol, summary);
        self.role_facts(map, symbol, summary);
        if needs.fact("local_callers") {
            map.insert("local_callers".to_owned(), self.local_callers(symbol));
        }
        if needs.fact("touched_by_tests") {
            map.insert(
                "touched_by_tests".to_owned(),
                json!(touched_by_tests(project, symbol)),
            );
        }
        if needs.fact("implementers") {
            let types = if symbol.kind == SymbolKind::Interface {
                project.implementers(&symbol.id)
            } else {
                &[]
            };
            let found: Vec<Value> = types
                .iter()
                .filter_map(|id| project.symbol(id))
                .map(|ty| {
                    let stateless = !project.members(&ty.id).iter().any(|m| {
                        project
                            .symbol(m)
                            .is_some_and(|m| m.kind == SymbolKind::Field)
                    });
                    json!({ "id": ty.id.as_str(), "name": ty.name, "stateless": stateless })
                })
                .collect();
            map.insert("implementers".to_owned(), json!(found));
        }
        if needs.fact("data_only") {
            map.insert("data_only".to_owned(), json!(data_only(project, symbol)));
        }
        if needs.fact("module_tested") {
            map.insert(
                "module_tested".to_owned(),
                json!(module_tested(project, symbol.id.module())),
            );
        }
        if needs.fact("file_first_test") {
            map.insert(
                "file_first_test".to_owned(),
                self.first_test(symbol).unwrap_or_else(|| json!({})),
            );
        }
        if needs.fact("gap") {
            map.insert("gap".to_owned(), self.gap(symbol));
        }
        if needs.fact("helper_user") {
            map.insert("helper_user".to_owned(), self.helper_user(symbol));
        }
    }

    /// The facts about where a symbol sits among the types, modules and tests
    /// of the project.
    fn placement_facts(&self, map: &mut Map<String, Value>, symbol: &Symbol) {
        let needs = self.needs;
        if needs.fact("envy") {
            map.insert("envy".to_owned(), self.envy(symbol));
        }
        if needs.fact("owner_home") {
            map.insert("owner_home".to_owned(), self.owner_home(symbol));
        }
        if needs.fact("module_imports") {
            map.insert("module_imports".to_owned(), self.module_imports(symbol));
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
        let ctx = KeyContext {
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

    fn metrics(&self, symbol: &Symbol) -> Value {
        let found = self.measures.as_ref().and_then(|m| m.get(&symbol.id));
        let mut value = json!(found.copied().unwrap_or_default());
        value["measured"] = json!(found.is_some());
        value
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

/// The strings of an option that is a list of them; empty when it is not.
fn strings(option: Option<&Value>) -> Vec<String> {
    option
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
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
