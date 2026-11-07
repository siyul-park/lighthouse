//! The code model as the graph holds it: each answer is read from the parts
//! of the model that the graph keeps, `resolved` and `incoming` of the files
//! it concerns and the table of symbols, so that the reads are recorded as
//! dependencies of the query that makes the view. What a view hands out is
//! borrowed from what it pulled, which an arena keeps for as long as it lives.

use std::{
    cell::{OnceCell, RefCell},
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use lighthouse_graph::{Error, Graph};
use typed_arena::Arena;

use super::{
    Boxed, Comment, DECLARATIONS, Declarations, EDGES, Edge, Edges, File, FunctionSummary,
    INCOMING, Incoming, LoadNeed, MODULE_CASES, Model, Module, Node, PATHS, Place, RESOLVED,
    Resolved, SITES, SYMBOLS, Site, Sites, Symbol, SymbolId, Table, TestCase, Tie, When, key,
    kinds::DUPLICATED, load_needs,
};

/// The model of the project as the graph holds it.
pub struct View<'v> {
    graph: &'v Graph,
    held: &'v Held,
    table: OnceCell<&'v Table>,
    resolved: RefCell<HashMap<PathBuf, &'v Resolved>>,
    incoming: RefCell<HashMap<PathBuf, &'v Incoming>>,
    declared_in: RefCell<HashMap<PathBuf, &'v Declarations>>,
    homes: RefCell<HashMap<SymbolId, Rc<Home<'v>>>>,
    files: RefCell<HashMap<String, Option<&'v File>>>,
    declared: RefCell<HashMap<String, Rc<Vec<usize>>>>,
    commented: RefCell<HashMap<String, Rc<Vec<usize>>>>,
    cases: RefCell<HashMap<String, &'v [TestCase]>>,
    sited: OnceCell<&'v Sites>,
    resolved_all: OnceCell<()>,
    declared_all: OnceCell<()>,
    everything: Everything<'v>,
    failed: RefCell<Option<Error>>,
}

/// What a view found of one symbol: where it is declared and what is said of
/// it, each found when first asked for, for the rest of the view.
struct Home<'v> {
    file: Option<PathBuf>,
    symbol: OnceCell<Option<&'v Symbol>>,
    place: OnceCell<Option<&'v Place>>,
    function: OnceCell<Option<&'v FunctionSummary>>,
    test: OnceCell<Option<&'v TestCase>>,
    tie: OnceCell<Option<&'v Tie>>,
}

/// What the whole project holds, found when first asked for.
#[derive(Default)]
struct Everything<'v> {
    symbols: OnceCell<Vec<&'v Symbol>>,
    functions: OnceCell<Vec<&'v FunctionSummary>>,
    files: OnceCell<Vec<&'v File>>,
    edges: OnceCell<(Vec<&'v Edge>, BTreeMap<usize, PathBuf>)>,
}

/// What the views of one query pull, kept until the query is done.
#[derive(Default)]
pub struct Held {
    resolved: Arena<Arc<Resolved>>,
    incoming: Arena<Arc<Incoming>>,
    tables: Arena<Arc<Table>>,
    declarations: Arena<Arc<Declarations>>,
    sites: Arena<Arc<Sites>>,
    edges: Arena<Arc<Edges>>,
    cases: Arena<Arc<Vec<TestCase>>>,
    files: Arena<File>,
}

impl<'v> View<'v> {
    /// The view of the project `graph` holds.
    pub fn new(graph: &'v Graph, held: &'v Held) -> Result<Self, Error> {
        Ok(Self {
            graph,
            held,
            table: OnceCell::new(),
            resolved: RefCell::default(),
            incoming: RefCell::default(),
            declared_in: RefCell::default(),
            homes: RefCell::default(),
            files: RefCell::default(),
            declared: RefCell::default(),
            commented: RefCell::default(),
            cases: RefCell::default(),
            sited: OnceCell::new(),
            resolved_all: OnceCell::new(),
            declared_all: OnceCell::new(),
            everything: Everything::default(),
            failed: RefCell::new(None),
        })
    }

    /// What went wrong in reading the graph, if anything: what could not be
    /// read reads as empty.
    pub fn failure(&self) -> Option<Error> {
        self.failed.borrow_mut().take()
    }

    /// The symbols of every file, one per id, sorted by id.
    fn all_symbols(&self) -> &[&'v Symbol] {
        self.everything.symbols.get_or_init(|| {
            let mut all: Vec<&Symbol> = self
                .declared_paths()
                .iter()
                .flat_map(|path| {
                    let path = Path::new(path);
                    let declared = self.declarations(path);
                    let at = self.declared(path);
                    (0..at.len())
                        .filter_map(move |n| declared.map(|d| &d.symbols[at[n]]))
                        .collect::<Vec<_>>()
                })
                .collect();
            all.sort_by(|a, b| a.id.cmp(&b.id));
            all.dedup_by(|a, b| a.id == b.id);
            all
        })
    }

    /// The edges of every file's slice that repeat none, and the file of those
    /// that start at a module and come from a fragment about one file.
    fn all_edges(&self) -> &(Vec<&'v Edge>, BTreeMap<usize, PathBuf>) {
        self.everything.edges.get_or_init(|| {
            let edges = self.keep(self.graph.query(&EDGES, &()));
            let edges = edges.map(|edges| &**self.held.edges.alloc(edges));
            let (mut all, mut origins) = (Vec::new(), BTreeMap::new());
            for path in self.resolved_paths() {
                let Some(resolved) = self.resolved(Path::new(&path)) else {
                    continue;
                };
                let dropped = edges
                    .and_then(|edges| edges.dropped.get(Path::new(&path)))
                    .map_or(&[][..], Vec::as_slice);
                let mut files = resolved
                    .slice
                    .runs
                    .iter()
                    .flat_map(|(len, file)| std::iter::repeat_n(file.as_ref(), *len));
                for (at, edge) in resolved.slice.fragment.edges.iter().enumerate() {
                    let file = files.next().flatten();
                    if dropped.binary_search(&(at as u32)).is_ok() {
                        continue;
                    }
                    if let (Node::Module(_), Some(file)) = (&edge.from, file) {
                        origins.insert(all.len(), file.clone());
                    }
                    all.push(edge);
                }
            }
            (all, origins)
        })
    }

    /// [`View::paths`], with what the files declare pulled on every thread the
    /// first time: one after the other is what a read of the whole project
    /// would wait for. A read of the declarations does not depend on where
    /// they sit.
    fn declared_paths(&self) -> Vec<String> {
        let paths = self.paths();
        self.declared_all.get_or_init(|| {
            self.graph.par_map(paths.clone(), |graph, path| {
                let _ = graph.query(&DECLARATIONS, path);
            });
        });
        paths
    }

    /// [`View::paths`], with the resolved slices pulled likewise.
    fn resolved_paths(&self) -> Vec<String> {
        let paths = self.paths();
        self.resolved_all.get_or_init(|| {
            self.graph.par_map(paths.clone(), |graph, path| {
                let _ = graph.query(&RESOLVED, path);
            });
        });
        paths
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
        if let Some(declared) = self.declarations(path) {
            let symbols = &declared.symbols;
            let duplicated = self
                .keep(self.graph.query(&DUPLICATED, &()))
                .unwrap_or_default();
            at = (0..symbols.len())
                .filter(|&i| {
                    !duplicated.contains(&symbols[i].id)
                        || self
                            .table()
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
        let merged = self.declarations(path).and_then(|declared| {
            let mut files = declared.files.iter().filter(|file| file.path == path);
            let mut kept = files.next()?.clone();
            for later in files {
                kept.facts.extend(later.facts.clone());
            }
            Some(&*self.held.files.alloc(kept))
        });
        self.files.borrow_mut().insert(key, merged);
        merged
    }

    /// What the rest of the project says of `id`.
    fn tie(&self, id: &SymbolId) -> Option<&'v Tie> {
        let home = self.home(id);
        *home.tie.get_or_init(|| {
            let file = home.file.as_deref()?;
            self.incoming(file)?.by_symbol.get(id)
        })
    }

    /// What the view found of `id`.
    fn home(&self, id: &SymbolId) -> Rc<Home<'v>> {
        if let Some(found) = self.homes.borrow().get(id) {
            return Rc::clone(found);
        }
        let file = self.table().entry(id).map(|entry| entry.file.clone());
        let home = Rc::new(Home {
            file,
            symbol: OnceCell::new(),
            place: OnceCell::new(),
            function: OnceCell::new(),
            test: OnceCell::new(),
            tie: OnceCell::new(),
        });
        self.homes.borrow_mut().insert(id.clone(), Rc::clone(&home));
        home
    }

    fn resolved(&self, path: &Path) -> Option<&'v Resolved> {
        if let Some(found) = self.resolved.borrow().get(path) {
            return Some(found);
        }
        let resolved = self.keep(self.graph.query(&RESOLVED, &key(path)))?;
        let resolved = &**self.held.resolved.alloc(resolved);
        self.resolved.borrow_mut().insert(path.to_owned(), resolved);
        Some(resolved)
    }

    /// What the file at `path` declares.
    fn declarations(&self, path: &Path) -> Option<&'v Declarations> {
        if let Some(found) = self.declared_in.borrow().get(path) {
            return Some(found);
        }
        let declared = self.keep(self.graph.query(&DECLARATIONS, &key(path)))?;
        let declared = &**self.held.declarations.alloc(declared);
        self.declared_in
            .borrow_mut()
            .insert(path.to_owned(), declared);
        Some(declared)
    }

    fn incoming(&self, path: &Path) -> Option<&'v Incoming> {
        if let Some(found) = self.incoming.borrow().get(path) {
            return Some(found);
        }
        let incoming = self.keep(self.graph.query(&INCOMING, &key(path)))?;
        let incoming = &**self.held.incoming.alloc(incoming);
        self.incoming.borrow_mut().insert(path.to_owned(), incoming);
        Some(incoming)
    }

    /// The table of symbols, read when first asked for: a rule that reads no
    /// symbol does not depend on them.
    fn table(&self) -> &'v Table {
        self.table.get_or_init(|| {
            let table = self
                .keep(self.graph.query(&SYMBOLS, &()))
                .unwrap_or_default();
            &**self.held.tables.alloc(table)
        })
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
        let home = self.home(id);
        *home.symbol.get_or_init(|| {
            self.declarations(home.file.as_deref()?)?
                .symbols
                .iter()
                .find(|symbol| symbol.id == *id)
        })
    }

    fn place(&self, id: &SymbolId) -> Option<&Place> {
        let home = self.home(id);
        *home.place.get_or_init(|| {
            self.resolved(home.file.as_deref()?)?
                .slice
                .fragment
                .places
                .iter()
                .find(|place| place.symbol == *id)
        })
    }

    fn symbols_in(&self, path: &Path) -> Boxed<'_, Symbol> {
        let Some(declared) = self.declarations(path) else {
            return Box::new(std::iter::empty());
        };
        let at = self.declared(path);
        Box::new((0..at.len()).map(move |n| &declared.symbols[at[n]]))
    }

    fn module(&self, path: &str) -> Option<&Module> {
        self.table().module(path)
    }

    fn function(&self, id: &SymbolId) -> Option<&FunctionSummary> {
        let home = self.home(id);
        *home.function.get_or_init(|| {
            self.resolved(home.file.as_deref()?)?
                .slice
                .fragment
                .functions
                .iter()
                .find(|function| function.symbol == *id)
        })
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
        let home = self.home(id);
        *home.test.get_or_init(|| {
            self.resolved(home.file.as_deref()?)?
                .slice
                .fragment
                .tests
                .iter()
                .find(|test| test.symbol == *id)
        })
    }

    fn tests_in(&self, module: &str) -> Boxed<'_, TestCase> {
        let found = self.cases.borrow().get(module).copied();
        let cases = match found {
            Some(cases) => cases,
            None => {
                let cases: &'v [TestCase] = self
                    .keep(self.graph.query(&MODULE_CASES, &module.to_owned()))
                    .map_or(&[], |cases| &**self.held.cases.alloc(cases));
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
        let Some(resolved) = self
            .home(id)
            .file
            .as_deref()
            .and_then(|file| self.resolved(file))
        else {
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
                    Node::Symbol(id) => self
                        .table()
                        .entry(id)
                        .is_some_and(|entry| entry.file == file),
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
                .resolved_paths()
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
        Box::new(self.table().modules())
    }

    fn files(&self) -> Boxed<'_, File> {
        let all = self.everything.files.get_or_init(|| {
            self.declared_paths()
                .iter()
                .filter_map(|path| self.merged(Path::new(path)))
                .collect()
        });
        Box::new(all.iter().copied())
    }

    fn files_in<'a>(&'a self, dir: &'a Path) -> Boxed<'a, File> {
        let paths = self.keep(self.graph.input(&PATHS, &())).flatten();
        let paths = paths.map_or_else(Vec::new, |paths| paths.as_ref().clone());
        let found: Vec<&File> = paths
            .iter()
            .map(Path::new)
            .filter(|path| path.parent() == Some(dir))
            .filter_map(|path| self.merged(path))
            .collect();
        Box::new(found.into_iter())
    }

    fn edges(&self) -> Boxed<'_, Edge> {
        Box::new(self.all_edges().0.iter().copied())
    }

    fn origin(&self, at: usize) -> Option<&Path> {
        self.all_edges().1.get(&at).map(PathBuf::as_path)
    }
}
