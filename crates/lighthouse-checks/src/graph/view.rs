//! The code model as the graph holds it: each answer is read from the parts
//! of the model that the graph keeps, `resolved` and `incoming` of the files
//! it concerns and the table of symbols, so that the reads are recorded as
//! dependencies of the query that makes the view. What a view hands out is
//! borrowed from what it pulled, which an arena keeps for as long as it lives.

use std::{
    cell::{OnceCell, RefCell},
    collections::HashMap,
    path::Path,
    rc::Rc,
    sync::Arc,
};

use lighthouse_engine::graph::{EDGES, INCOMING, PATHS, RESOLVED, SITES, SYMBOLS};
use lighthouse_graph::{Error, Graph};
use lighthouse_model::ucm::{
    Boxed, Comment, Edge, Edges, File, FunctionSummary, Incoming, LoadNeed, Model, Module, Node,
    Resolved, Site, Sites, Symbol, SymbolId, Table, TestCase, When, load_needs,
};
use lighthouse_spec::key;
use typed_arena::Arena;

use super::kinds::{CASES, Entry, INDEX};

/// The model of the project as the graph holds it.
pub struct View<'v> {
    graph: &'v Graph,
    held: &'v Held,
    table: &'v Table,
    resolved: RefCell<HashMap<String, &'v Resolved>>,
    incoming: RefCell<HashMap<String, &'v Incoming>>,
    files: RefCell<HashMap<String, Option<&'v File>>>,
    declared: RefCell<HashMap<String, Rc<Vec<usize>>>>,
    commented: RefCell<HashMap<String, Rc<Vec<usize>>>>,
    cases: RefCell<HashMap<String, &'v [TestCase]>>,
    sited: OnceCell<&'v Sites>,
    everything: Everything<'v>,
    failed: RefCell<Option<Error>>,
}

/// What the whole project holds, found when first asked for.
#[derive(Default)]
struct Everything<'v> {
    symbols: OnceCell<Vec<&'v Symbol>>,
    functions: OnceCell<Vec<&'v FunctionSummary>>,
    files: OnceCell<Vec<&'v File>>,
    edges: OnceCell<Vec<&'v Edge>>,
}

/// What the views of one query pull, kept until the query is done.
#[derive(Default)]
pub struct Held {
    resolved: Arena<Arc<Resolved>>,
    incoming: Arena<Arc<Incoming>>,
    tables: Arena<Arc<Table>>,
    sites: Arena<Arc<Sites>>,
    edges: Arena<Arc<Edges>>,
    entries: Arena<Arc<Entry>>,
    files: Arena<File>,
}

impl<'v> View<'v> {
    /// The view of the project `graph` holds.
    pub fn new(graph: &'v Graph, held: &'v Held) -> Result<Self, Error> {
        let table = &**held.tables.alloc(graph.query(&SYMBOLS, &())?);
        Ok(Self {
            graph,
            held,
            table,
            resolved: RefCell::default(),
            incoming: RefCell::default(),
            files: RefCell::default(),
            declared: RefCell::default(),
            commented: RefCell::default(),
            cases: RefCell::default(),
            sited: OnceCell::new(),
            everything: Everything::default(),
            failed: RefCell::new(None),
        })
    }

    /// What went wrong in reading the graph, if anything: what could not be
    /// read reads as empty.
    pub fn failure(&self) -> Option<Error> {
        self.failed.borrow_mut().take()
    }

    /// The tests of every file, one per symbol, sorted by symbol.
    pub fn tests(&self) -> Vec<&'v TestCase> {
        let mut all: Vec<&TestCase> = self
            .paths()
            .iter()
            .filter_map(|path| self.resolved(Path::new(path)))
            .flat_map(|resolved| &resolved.slice.fragment.tests)
            .collect();
        all.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        all.dedup_by(|a, b| a.symbol == b.symbol);
        all
    }

    /// The symbols of every file, one per id, sorted by id.
    fn all_symbols(&self) -> &[&'v Symbol] {
        self.everything.symbols.get_or_init(|| {
            let mut all: Vec<&Symbol> = self
                .paths()
                .iter()
                .flat_map(|path| {
                    let path = Path::new(path);
                    let resolved = self.resolved(path);
                    let at = self.declared(path);
                    (0..at.len())
                        .filter_map(move |n| resolved.map(|r| &r.slice.fragment.symbols[at[n]]))
                        .collect::<Vec<_>>()
                })
                .collect();
            all.sort_by(|a, b| a.id.cmp(&b.id));
            all.dedup_by(|a, b| a.id == b.id);
            all
        })
    }

    /// The paths of the project's files.
    fn paths(&self) -> Vec<String> {
        let paths = self.keep(self.graph.input(&PATHS, &())).flatten();
        paths.map_or_else(Vec::new, |paths| paths.as_ref().clone())
    }

    /// The positions among the symbols of `path`'s slice of those it declares
    /// itself, one per id, sorted by id.
    fn declared(&self, path: &Path) -> Rc<Vec<usize>> {
        let key = key(path);
        if let Some(found) = self.declared.borrow().get(&key) {
            return Rc::clone(found);
        }
        let mut at: Vec<usize> = Vec::new();
        if let Some(resolved) = self.resolved(path) {
            let symbols = &resolved.slice.fragment.symbols;
            at = (0..symbols.len())
                .filter(|&i| {
                    self.table
                        .entry(&symbols[i].id)
                        .is_some_and(|entry| entry.file == path)
                })
                .collect();
            at.sort_by(|&a, &b| symbols[a].id.cmp(&symbols[b].id));
            at.dedup_by(|a, b| symbols[*a].id == symbols[*b].id);
        }
        let at = Rc::new(at);
        self.declared.borrow_mut().insert(key, Rc::clone(&at));
        at
    }

    /// The positions among the comments of `path`'s slice of those that are
    /// its own, in source order, without repeats.
    fn commented(&self, path: &Path) -> Rc<Vec<usize>> {
        let key = key(path);
        if let Some(found) = self.commented.borrow().get(&key) {
            return Rc::clone(found);
        }
        let mut at: Vec<usize> = Vec::new();
        if let Some(resolved) = self.resolved(path) {
            let comments = &resolved.slice.fragment.comments;
            at = (0..comments.len())
                .filter(|&i| comments[i].file == path)
                .collect();
            at.sort_by(|&a, &b| comments[a].span.start.cmp(&comments[b].span.start));
            at.dedup_by(|a, b| comments[*a] == comments[*b]);
        }
        let at = Rc::new(at);
        self.commented.borrow_mut().insert(key, Rc::clone(&at));
        at
    }

    /// The file at `path`, as its slice and the slices of other languages of
    /// its regions say of it.
    fn merged(&self, path: &Path) -> Option<&'v File> {
        let key = key(path);
        if let Some(found) = self.files.borrow().get(&key) {
            return *found;
        }
        let merged = self.resolved(path).and_then(|resolved| {
            let mut files = resolved
                .slice
                .fragment
                .files
                .iter()
                .filter(|file| file.path == path);
            let mut kept = files.next()?.clone();
            for later in files {
                kept.facts.extend(later.facts.clone());
            }
            Some(&*self.held.files.alloc(kept))
        });
        self.files.borrow_mut().insert(key, merged);
        merged
    }

    /// The part of the project that `id` is declared in.
    fn home(&self, id: &SymbolId) -> Option<&'v Resolved> {
        self.resolved(&self.table.entry(id)?.file)
    }

    /// What the rest of the project says of `id`.
    fn tie(&self, id: &SymbolId) -> Option<&'v lighthouse_model::ucm::Tie> {
        self.incoming(&self.table.entry(id)?.file)?
            .by_symbol
            .get(id)
    }

    fn resolved(&self, path: &Path) -> Option<&'v Resolved> {
        let key = key(path);
        if let Some(found) = self.resolved.borrow().get(&key) {
            return Some(found);
        }
        let resolved = self.keep(self.graph.query(&RESOLVED, &key))?;
        let resolved = &**self.held.resolved.alloc(resolved);
        self.resolved.borrow_mut().insert(key, resolved);
        Some(resolved)
    }

    fn incoming(&self, path: &Path) -> Option<&'v Incoming> {
        let key = key(path);
        if let Some(found) = self.incoming.borrow().get(&key) {
            return Some(found);
        }
        let incoming = self.keep(self.graph.query(&INCOMING, &key))?;
        let incoming = &**self.held.incoming.alloc(incoming);
        self.incoming.borrow_mut().insert(key, incoming);
        Some(incoming)
    }

    fn entry(&self, name: &str, key: &str) -> Option<&'v Entry> {
        let entry = self.keep(self.graph.query(&INDEX, &(name.to_owned(), key.to_owned())))?;
        Some(&**self.held.entries.alloc(entry))
    }

    fn keep<T>(&self, found: Result<T, Error>) -> Option<T> {
        match found {
            Ok(found) => Some(found),
            Err(error) => {
                self.failed.borrow_mut().get_or_insert(error);
                None
            }
        }
    }
}

impl<'v> Model for View<'v> {
    fn file(&self, path: &Path) -> Option<&File> {
        self.merged(path)
    }

    fn symbol(&self, id: &SymbolId) -> Option<&Symbol> {
        self.home(id)?
            .slice
            .fragment
            .symbols
            .iter()
            .find(|symbol| symbol.id == *id)
    }

    fn symbols_in(&self, path: &Path) -> Boxed<'_, Symbol> {
        let Some(resolved) = self.resolved(path) else {
            return Box::new(std::iter::empty());
        };
        let at = self.declared(path);
        Box::new((0..at.len()).map(move |n| &resolved.slice.fragment.symbols[at[n]]))
    }

    fn module(&self, path: &str) -> Option<&Module> {
        self.table.module(path)
    }

    fn function(&self, id: &SymbolId) -> Option<&FunctionSummary> {
        self.home(id)?
            .slice
            .fragment
            .functions
            .iter()
            .find(|function| function.symbol == *id)
    }

    fn callers(&self, id: &SymbolId) -> &[SymbolId] {
        self.tie(id).map_or(&[], |tie| &tie.callers)
    }

    fn references(&self, id: &SymbolId) -> &[SymbolId] {
        self.tie(id).map_or(&[], |tie| &tie.references)
    }

    fn uses(&self, id: &SymbolId) -> &[SymbolId] {
        self.tie(id).map_or(&[], |tie| &tie.uses)
    }

    fn members(&self, id: &SymbolId) -> &[SymbolId] {
        self.tie(id).map_or(&[], |tie| &tie.members)
    }

    fn test(&self, id: &SymbolId) -> Option<&TestCase> {
        self.home(id)?
            .slice
            .fragment
            .tests
            .iter()
            .find(|test| test.symbol == *id)
    }

    fn tests_in(&self, module: &str) -> Boxed<'_, TestCase> {
        let found = self.cases.borrow().get(module).copied();
        let cases = match found {
            Some(cases) => cases,
            None => {
                let cases: &'v [TestCase] = match self.entry(CASES, module) {
                    Some(Entry::Cases(cases)) => cases,
                    _ => &[],
                };
                self.cases.borrow_mut().insert(module.to_owned(), cases);
                cases
            }
        };
        Box::new(cases.iter())
    }

    fn sites(&self, id: &SymbolId) -> &[Site] {
        let sites = self.sited.get_or_init(|| {
            let sites = self.keep(self.graph.query(&SITES, &()));
            let sites = sites.unwrap_or_default();
            &**self.held.sites.alloc(sites)
        });
        sites.0.get(id).map_or(&[], Vec::as_slice)
    }

    fn comments_in(&self, path: &Path) -> Boxed<'_, Comment> {
        let Some(resolved) = self.resolved(path) else {
            return Box::new(std::iter::empty());
        };
        let at = self.commented(path);
        Box::new((0..at.len()).map(move |n| &resolved.slice.fragment.comments[at[n]]))
    }

    fn edges_from(&self, id: &SymbolId) -> Boxed<'_, Edge> {
        let Some(resolved) = self.home(id) else {
            return Box::new(std::iter::empty());
        };
        let id = id.clone();
        Box::new(
            resolved
                .slice
                .fragment
                .edges
                .iter()
                .filter(move |edge| matches!(&edge.from, Node::Symbol(from) if *from == id)),
        )
    }

    fn implementers(&self, id: &SymbolId) -> &[SymbolId] {
        self.tie(id).map_or(&[], |tie| &tie.implementers)
    }

    fn callees(&self, id: &SymbolId) -> &[SymbolId] {
        self.tie(id).map_or(&[], |tie| &tie.callees)
    }

    fn load_needs(&self, file: &Path) -> Vec<LoadNeed> {
        let Some(resolved) = self.resolved(file) else {
            return Vec::new();
        };
        let slice = &resolved.slice;
        let mut runs = slice
            .runs
            .iter()
            .flat_map(|(len, run)| std::iter::repeat_n(run.as_ref(), *len));
        let loads = slice.fragment.edges.iter().filter(|edge| {
            let run = runs.next().flatten();
            edge.when == When::Load
                && match &edge.from {
                    Node::Symbol(id) => {
                        self.table.entry(id).is_some_and(|entry| entry.file == file)
                    }
                    Node::Module(_) => run.is_some_and(|run| run == file),
                }
        });
        load_needs(self, file, loads)
    }

    fn symbols(&self) -> Boxed<'_, Symbol> {
        Box::new(self.all_symbols().iter().copied())
    }

    fn functions(&self) -> Boxed<'_, FunctionSummary> {
        let all = self.everything.functions.get_or_init(|| {
            let mut all: Vec<&FunctionSummary> = self
                .paths()
                .iter()
                .filter_map(|path| self.resolved(Path::new(path)))
                .flat_map(|resolved| &resolved.slice.fragment.functions)
                .collect();
            all.sort_by(|a, b| a.symbol.cmp(&b.symbol));
            all.dedup_by(|a, b| a.symbol == b.symbol);
            all
        });
        Box::new(all.iter().copied())
    }

    fn modules(&self) -> Boxed<'_, Module> {
        Box::new(self.table.modules())
    }

    fn files(&self) -> Boxed<'_, File> {
        let all = self.everything.files.get_or_init(|| {
            self.paths()
                .iter()
                .filter_map(|path| self.merged(Path::new(path)))
                .collect()
        });
        Box::new(all.iter().copied())
    }

    fn edges(&self) -> Boxed<'_, Edge> {
        let all = self.everything.edges.get_or_init(|| {
            let edges = self.keep(self.graph.query(&EDGES, &()));
            let edges = edges.map(|edges| &**self.held.edges.alloc(edges));
            let mut all = Vec::new();
            for path in self.paths() {
                let Some(resolved) = self.resolved(Path::new(&path)) else {
                    continue;
                };
                let dropped = edges
                    .and_then(|edges| edges.dropped.get(Path::new(&path)))
                    .map_or(&[][..], Vec::as_slice);
                for (at, edge) in resolved.slice.fragment.edges.iter().enumerate() {
                    if dropped.binary_search(&(at as u32)).is_err() {
                        all.push(edge);
                    }
                }
            }
            all
        });
        Box::new(all.iter().copied())
    }
}
