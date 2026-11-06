//! What a rule or a fact asks of the code model, as a trait: the merged
//! [`Project`] answers from its indexes, and a model kept as queries of a
//! graph answers from the parts it pulls, so that what is read is recorded.

use std::path::Path;

use super::{
    Comment, Edge, File, FunctionSummary, LoadNeed, LoadPoint, Module, Project, Site, Symbol,
    SymbolId, TestCase, load_order,
};
use crate::{Diagnostic, Position};

/// A walk over some of the model.
pub type Boxed<'a, T> = Box<dyn Iterator<Item = &'a T> + 'a>;

/// The code model: what is declared, and how the declarations are tied.
pub trait Model {
    /// The file at this exact project-relative path.
    fn file(&self, path: &Path) -> Option<&File>;

    /// The symbol with this id, if declared.
    fn symbol(&self, id: &SymbolId) -> Option<&Symbol>;

    /// Symbols declared in `path`, sorted by id.
    fn symbols_in(&self, path: &Path) -> Boxed<'_, Symbol>;

    /// The module with this path.
    fn module(&self, path: &str) -> Option<&Module>;

    /// The summary of a function or method symbol; `None` for other kinds.
    fn function(&self, id: &SymbolId) -> Option<&FunctionSummary>;

    /// Distinct symbols with a resolved, non-heuristic `calls` edge to `id`,
    /// excluding itself.
    fn callers(&self, id: &SymbolId) -> &[SymbolId];

    /// Distinct symbols with a resolved `references` edge to `id`, excluding
    /// itself: every use of `id` as a value rather than a call. Heuristic edges
    /// are included: they say a symbol might be used.
    fn references(&self, id: &SymbolId) -> &[SymbolId];

    /// Distinct symbols `id` calls or references through a resolved,
    /// non-heuristic edge, excluding itself: every use `id` makes of another
    /// symbol, whether as a call or as a value.
    fn uses(&self, id: &SymbolId) -> &[SymbolId];

    /// Symbols whose `owner` is `id`: fields, methods, associated items.
    fn members(&self, id: &SymbolId) -> &[SymbolId];

    /// The test case of a test symbol.
    fn test(&self, id: &SymbolId) -> Option<&TestCase>;

    /// Test cases declared in `module`, ordered by id.
    fn tests_in(&self, module: &str) -> Boxed<'_, TestCase>;

    /// Every place that refers to `id` through an edge that has a site: calls,
    /// references and the other relations alike, in file order. Empty when the
    /// provider reports no reference sites.
    fn sites(&self, id: &SymbolId) -> &[Site];

    /// Comments of `path`, in source order.
    fn comments_in(&self, path: &Path) -> Boxed<'_, Comment>;

    /// The edges that start at symbol `id`, in the project's edge order.
    fn edges_from(&self, id: &SymbolId) -> Boxed<'_, Edge>;

    /// The file the edge at position `at` of `edges` comes from, when it starts
    /// at a module and its fragment is about one file; an edge that starts at
    /// a symbol comes from the file of the symbol.
    fn origin(&self, at: usize) -> Option<&Path>;

    /// The types that implement the interface `id`, in test files too.
    fn implementers(&self, id: &SymbolId) -> &[SymbolId];

    /// Distinct symbols `id` has a resolved, non-heuristic `calls` edge to,
    /// excluding itself.
    fn callees(&self, id: &SymbolId) -> &[SymbolId];

    /// The needs of `file` as it loads, in a fixed order; none for a file of a
    /// language that does not order its declarations. Top-level code that
    /// needs a binding more than once counts at the use the project keeps (the
    /// first its provider listed, which is the earliest).
    fn load_needs(&self, file: &Path) -> Vec<LoadNeed>;

    /// Every symbol, sorted by id.
    fn symbols(&self) -> Boxed<'_, Symbol>;

    /// Every function summary, sorted by symbol.
    fn functions(&self) -> Boxed<'_, FunctionSummary>;

    /// Every module, sorted by path.
    fn modules(&self) -> Boxed<'_, Module>;

    /// Every file, sorted by path.
    fn files(&self) -> Boxed<'_, File>;

    /// The files directly in the directory `dir`, sorted by path: a reader of
    /// them does not depend on the files of the rest of the project.
    fn files_in<'a>(&'a self, dir: &'a Path) -> Boxed<'a, File>;

    /// Every edge, in the project's order.
    fn edges(&self) -> Boxed<'_, Edge>;

    /// Whether the symbol is test code: declared in a test file, or in a module
    /// that exists to test another (`test_of`), such as an inline `#[cfg(test)]`
    /// module in a production file.
    fn in_test(&self, id: &SymbolId) -> bool {
        let in_test_file = self
            .symbol(id)
            .and_then(|s| self.file(&s.file))
            .is_some_and(|f| f.test);
        in_test_file
            || self
                .module(id.module())
                .is_some_and(|m| m.test_of.is_some())
    }

    /// Whether the symbol is example code, which documentation shows and the
    /// project does not use: declared in a module that example code declares,
    /// or in a host file all of whose regions are example code (a fence in
    /// the package of a project module declares no module of its own).
    fn in_example(&self, id: &SymbolId) -> bool {
        self.module(id.module()).is_some_and(|m| m.example)
            || self
                .symbol(id)
                .and_then(|symbol| self.file(&symbol.file))
                .is_some_and(File::is_example)
    }

    /// The language the symbol is in: the one its module's host stated, else
    /// the language analyzed in its file; `None` for a file the model does not
    /// know.
    fn language_of(&self, symbol: &Symbol) -> Option<&str> {
        self.module(symbol.id.module())
            .and_then(|m| m.language.as_deref())
            .or_else(|| self.file(&symbol.file).map(File::analysis_language))
    }

    /// The needs of `file` that its text breaks: a binding that starts at or
    /// below the code that needs it.
    fn load_breaks(&self, file: &Path) -> Vec<LoadNeed> {
        self.load_needs(file)
            .into_iter()
            .filter(
                |need| match (self.load_place(&need.user), self.symbol(&need.binding)) {
                    (Some(user), Some(binding)) => load_order::start_of(binding) >= user,
                    _ => false,
                },
            )
            .collect()
    }

    /// Where the code of `point` starts.
    fn load_place(&self, point: &LoadPoint) -> Option<Position> {
        match point {
            LoadPoint::Declaration(id) => self.symbol(id).map(load_order::start_of),
            LoadPoint::Site(span) => Some(span.start),
        }
    }

    /// The symbol a finding is about, if it is in the finding's file, else the
    /// innermost one whose span holds the start of the finding. `None` for a
    /// finding no symbol covers, such as one about the file as a whole.
    fn anchor(&self, finding: &Diagnostic) -> Option<&Symbol> {
        finding
            .symbol
            .as_deref()
            .and_then(SymbolId::parse)
            .and_then(|id| self.symbol(&id))
            .filter(|symbol| symbol.file == finding.file)
            .or_else(|| self.enclosing(finding))
    }

    /// The symbol a finding is about, else the one that encloses or precedes
    /// it; empty when there is none. What tells apart findings that share a
    /// fingerprint ([`Diagnostic::disambiguate`]).
    fn discriminator(&self, finding: &Diagnostic) -> String {
        if let Some(symbol) = &finding.symbol {
            return symbol.clone();
        }
        let at = finding.span.start;
        let preceding = self
            .symbols_in(&finding.file)
            .filter(|s| s.span.end < at)
            .max_by_key(|s| s.span.end);
        self.enclosing(finding)
            .or(preceding)
            .map_or_else(String::new, |s| s.id.as_str().to_owned())
    }

    /// The innermost symbol whose span holds the start of the finding.
    fn enclosing(&self, finding: &Diagnostic) -> Option<&Symbol> {
        let at = finding.span.start;
        self.symbols_in(&finding.file)
            .filter(|s| s.span.start <= at && at <= s.span.end)
            .max_by_key(|s| s.span.start)
    }
}

impl Model for Project {
    fn origin(&self, at: usize) -> Option<&Path> {
        self.inner
            .index
            .origins
            .get(&at)
            .map(std::path::PathBuf::as_path)
    }

    fn file(&self, path: &Path) -> Option<&File> {
        let at = self
            .inner
            .all
            .files
            .binary_search_by(|f| f.path.as_path().cmp(path))
            .ok()?;
        self.inner.all.files.get(at)
    }

    fn symbol(&self, id: &SymbolId) -> Option<&Symbol> {
        self.inner
            .all
            .symbols
            .get(*self.inner.index.symbols.get(id)?)
    }

    fn symbols_in(&self, path: &Path) -> Boxed<'_, Symbol> {
        let at = self
            .inner
            .index
            .in_file
            .get(path)
            .map_or(&[][..], Vec::as_slice);
        Box::new(at.iter().map(|&i| &self.inner.all.symbols[i]))
    }

    fn module(&self, path: &str) -> Option<&Module> {
        let at = self
            .inner
            .all
            .modules
            .binary_search_by(|m| m.path.as_str().cmp(path))
            .ok()?;
        self.inner.all.modules.get(at)
    }

    fn function(&self, id: &SymbolId) -> Option<&FunctionSummary> {
        self.inner
            .all
            .functions
            .get(*self.inner.index.functions.get(id)?)
    }

    fn callers(&self, id: &SymbolId) -> &[SymbolId] {
        entries(&self.inner.index.reverse.callers, id)
    }

    fn references(&self, id: &SymbolId) -> &[SymbolId] {
        entries(&self.inner.index.reverse.references, id)
    }

    fn uses(&self, id: &SymbolId) -> &[SymbolId] {
        entries(&self.inner.index.reverse.uses, id)
    }

    fn members(&self, id: &SymbolId) -> &[SymbolId] {
        entries(&self.inner.index.reverse.members, id)
    }

    fn test(&self, id: &SymbolId) -> Option<&TestCase> {
        let at = self
            .inner
            .all
            .tests
            .binary_search_by(|t| t.symbol.cmp(id))
            .ok()?;
        self.inner.all.tests.get(at)
    }

    fn tests_in(&self, module: &str) -> Boxed<'_, TestCase> {
        let prefix = format!("{module}::");
        let tests = &self.inner.all.tests;
        let start = tests.partition_point(|t| t.symbol.as_str() < prefix.as_str());
        let len = tests[start..].partition_point(|t| t.symbol.as_str().starts_with(&prefix));
        Box::new(tests[start..start + len].iter())
    }

    fn sites(&self, id: &SymbolId) -> &[Site] {
        entries(&self.inner.index.sites.0, id)
    }

    fn comments_in(&self, path: &Path) -> Boxed<'_, Comment> {
        let comments = &self.inner.all.comments;
        let start = comments.partition_point(|c| c.file.as_path() < path);
        let end = comments.partition_point(|c| c.file.as_path() <= path);
        Box::new(comments[start..end].iter())
    }

    fn edges_from(&self, id: &SymbolId) -> Boxed<'_, Edge> {
        Box::new(
            self.inner
                .index
                .edges_from
                .get(id)
                .into_iter()
                .flatten()
                .map(|at| &self.inner.all.edges[*at]),
        )
    }

    fn implementers(&self, id: &SymbolId) -> &[SymbolId] {
        entries(&self.inner.index.reverse.implementers, id)
    }

    fn callees(&self, id: &SymbolId) -> &[SymbolId] {
        entries(&self.inner.index.reverse.callees, id)
    }

    fn load_needs(&self, file: &Path) -> Vec<LoadNeed> {
        let loads = self.inner.index.loads.get(file).into_iter().flatten();
        load_order::load_needs(self, file, loads.map(|at| &self.inner.all.edges[*at]))
    }

    fn symbols(&self) -> Boxed<'_, Symbol> {
        Box::new(self.inner.all.symbols.iter())
    }

    fn functions(&self) -> Boxed<'_, FunctionSummary> {
        Box::new(self.inner.all.functions.iter())
    }

    fn modules(&self) -> Boxed<'_, Module> {
        Box::new(self.inner.all.modules.iter())
    }

    fn files(&self) -> Boxed<'_, File> {
        Box::new(self.inner.all.files.iter())
    }

    fn files_in<'a>(&'a self, dir: &'a Path) -> Boxed<'a, File> {
        Box::new(
            self.inner
                .all
                .files
                .iter()
                .filter(move |file| file.path.parent() == Some(dir)),
        )
    }

    fn edges(&self) -> Boxed<'_, Edge> {
        Box::new(self.inner.all.edges.iter())
    }
}

/// The entries of `map` under `id`, none when it has none.
fn entries<'a, T>(map: &'a std::collections::BTreeMap<SymbolId, Vec<T>>, id: &SymbolId) -> &'a [T] {
    map.get(id).map_or(&[], Vec::as_slice)
}
