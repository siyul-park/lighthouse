use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet, HashSet},
    fmt,
    ops::Deref,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::Fingerprint;

/// 1-based line and column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: Position,
    pub end: Position,
}

/// Provider feature that rules may require.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    SemanticEdges,
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SemanticEdges => f.write_str("semantic-edges"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub path: PathBuf,
    pub lang: String,
    pub hash: String,
    pub generated: bool,
    pub test: bool,
}

/// Package, directory or namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Module {
    pub path: String,
    /// Name the language gives the module, such as a Go package name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The module whose public surface this one tests from outside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_of: Option<String>,
}

/// Project-stable identity: `module::owner::name#kind`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SymbolId(String);

impl SymbolId {
    pub fn new(module: &str, owners: &[&str], name: &str, kind: SymbolKind) -> Self {
        let mut id = module.to_owned();
        for part in owners.iter().chain([&name]) {
            id.push_str("::");
            id.push_str(part);
        }
        id.push('#');
        id.push_str(kind.as_str());
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Accepts an id built elsewhere, such as by a plugin: `module::name`
    /// parts ending in `#kind` with a known kind.
    pub fn parse(id: &str) -> Option<Self> {
        let (head, kind) = id.rsplit_once('#')?;
        let kind = SymbolKind::ALL.iter().any(|k| k.as_str() == kind);
        (kind && head.contains("::")).then(|| Self(id.to_owned()))
    }

    /// Path of the module that declares the symbol.
    pub fn module(&self) -> &str {
        self.0
            .split_once("::")
            .map_or(&self.0, |(module, _)| module)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolKind {
    Function,
    Method,
    Type,
    Field,
    Const,
    Var,
    Interface,
    Test,
}

impl SymbolKind {
    const ALL: [Self; 8] = [
        Self::Function,
        Self::Method,
        Self::Type,
        Self::Field,
        Self::Const,
        Self::Var,
        Self::Interface,
        Self::Test,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Type => "type",
            Self::Field => "field",
            Self::Const => "const",
            Self::Var => "var",
            Self::Interface => "interface",
            Self::Test => "test",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Visibility {
    Public,
    Private,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub id: SymbolId,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    pub owner: Option<SymbolId>,
    pub file: PathBuf,
    pub span: Span,
    pub doc: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Node {
    Module(String),
    Symbol(SymbolId),
}

/// Edge target. `Path` is resolved by [`Project::merge`]: a module path, a full
/// symbol id (`module::owner::name#kind`), or a kind-less id
/// (`module::owner::name`) that names exactly one symbol.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    Resolved(Node),
    Path(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Calls,
    References,
    Imports,
    Contains,
    Implements,
    /// Use of a member that its language keeps private, from outside the unit
    /// that owns it. What the unit is depends on the language: a type, a
    /// module, a package. Providers emit it only where such a use is possible;
    /// Go cannot reach an unexported member from another package, so the Go
    /// provider emits none.
    AccessesPrivate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    /// The target comes from type information.
    Semantic,
    /// The target comes from names alone.
    Syntactic,
    /// A guess: the source may use the target. Callers, callees and other
    /// counting analyses ignore such edges; [`Project::references`] keeps them
    /// so a rule asking "might this be used" fails safe.
    Heuristic,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Edge {
    pub kind: EdgeKind,
    pub from: Node,
    pub to: Target,
    pub resolution: Resolution,
}

/// Normalized control-flow construct, shared by every language. Providers map
/// their syntax onto these kinds; analyzers never see language syntax.
///
/// Mapping guidance for providers:
/// - conditional expression (`?:`, `x if c else y`): `If`; `elif`/`else if`:
///   `ElseIf`; `else`, also `for ... else` and `while ... else`: `Else`.
/// - `switch`, `match`, `select`: one `Switch` whose `arms` counts the arms
///   that are not the default or wildcard arm.
/// - every loop form, comprehension generators included: `Loop`; a
///   comprehension filter `if`: `If`.
/// - `catch`/`except`/`rescue` handler: one `Catch` each; `try` and `finally`
///   emit nothing.
/// - `goto` and labeled `break`/`continue`: `Jump`.
/// - a run of like `&&`/`||`/`and`/`or` operators: one `Logic` whose
///   `operators` counts the operators in the run; `??` and `?.` emit nothing.
/// - a direct call to the enclosing function: `Recursion`.
/// - nested functions, lambdas and closures emit nothing but raise the
///   nesting of what they contain.
///
/// Cyclomatic complexity is `1 + ifs + else-ifs + loops + catches + switch
/// arms + logic operators`; cognitive complexity follows Campbell 2018.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlowKind {
    If,
    ElseIf,
    Else,
    Switch,
    Loop,
    Catch,
    Jump,
    Logic,
    Recursion,
}

/// A construct and the number of enclosing nesting constructs (including
/// nested functions) around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flow {
    pub kind: FlowKind,
    pub nesting: u32,
    /// `Switch`: arms other than the default arm.
    #[serde(default)]
    pub arms: u32,
    /// `Logic`: operators in the run.
    #[serde(default)]
    pub operators: u32,
    /// `Switch`: every arm, default included, is a single return.
    #[serde(default)]
    pub returning: bool,
}

impl Flow {
    pub fn new(kind: FlowKind, nesting: u32) -> Self {
        Self {
            kind,
            nesting,
            arms: 0,
            operators: 0,
            returning: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSummary {
    pub symbol: SymbolId,
    /// Deepest level of nested constructs; a flat body is 0.
    pub max_nesting: u32,
    pub statements: u32,
    /// Statements directly in the body, not in nested blocks.
    pub top_level: u32,
    pub params: u32,
    pub returns: u32,
    /// Leaf tokens of the body.
    pub tokens: u32,
    /// Control-flow constructs in source order.
    pub flow: Vec<Flow>,
    /// Normalized AST/token fingerprint for clone detection.
    pub clone_fingerprint: Option<Fingerprint>,
    /// The body is a single call that passes the receiver and every parameter
    /// on, in order.
    pub forwards_to: Option<Target>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TestStyle {
    Table,
    Scenario,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestCase {
    pub symbol: SymbolId,
    pub nesting: u32,
    pub style: TestStyle,
    pub targets: Vec<Target>,
}

/// UCM contribution of one indexed file; ids are project-stable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fragment {
    pub files: Vec<File>,
    pub modules: Vec<Module>,
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
    pub functions: Vec<FunctionSummary>,
    pub tests: Vec<TestCase>,
}

/// All fragments merged: sorted, deduplicated, edge targets resolved.
///
/// A symbol id declared in several files (build variants, for instance) keeps
/// the first declaration in file order; the rest are dropped and reported by
/// [`Project::notices`], as are targets that stayed ambiguous.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Project {
    all: Fragment,
    index: Index,
    notices: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Index {
    symbols: BTreeMap<SymbolId, usize>,
    in_file: BTreeMap<PathBuf, Vec<usize>>,
    functions: BTreeMap<SymbolId, usize>,
    callers: BTreeMap<SymbolId, Vec<SymbolId>>,
    callees: BTreeMap<SymbolId, Vec<SymbolId>>,
    references: BTreeMap<SymbolId, Vec<SymbolId>>,
}

impl Deref for Project {
    type Target = Fragment;

    fn deref(&self) -> &Fragment {
        &self.all
    }
}

impl Project {
    pub fn merge(parts: impl IntoIterator<Item = Fragment>) -> Self {
        let mut all = Fragment::default();
        for part in parts {
            all.files.extend(part.files);
            all.modules.extend(part.modules);
            all.symbols.extend(part.symbols);
            all.edges.extend(part.edges);
            all.functions.extend(part.functions);
            all.tests.extend(part.tests);
        }
        all.files.sort_by(|a, b| a.path.cmp(&b.path));
        all.files.dedup_by(|a, b| a.path == b.path);
        all.modules.sort_by(|a, b| a.path.cmp(&b.path));
        all.modules.dedup_by(|a, b| a.path == b.path);
        all.symbols.sort_by(|a, b| a.id.cmp(&b.id));
        let mut notices = Vec::new();
        notices.extend(duplicate_notice(&all.symbols));
        all.symbols.dedup_by(|a, b| a.id == b.id);
        all.functions.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        all.functions.dedup_by(|a, b| a.symbol == b.symbol);
        all.tests.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        all.tests.dedup_by(|a, b| a.symbol == b.symbol);

        let resolver = Resolver::new(&all);
        let resolved: Vec<Target> = all
            .edges
            .iter()
            .map(|e| resolver.resolve(&e.to, e.kind == EdgeKind::Calls))
            .collect();
        let targets: Vec<Vec<Target>> = all
            .tests
            .iter()
            .map(|t| {
                t.targets
                    .iter()
                    .map(|x| resolver.resolve(x, false))
                    .collect()
            })
            .collect();
        let forwards: Vec<Option<Target>> = all
            .functions
            .iter()
            .map(|f| f.forwards_to.as_ref().map(|t| resolver.resolve(t, true)))
            .collect();
        let ambiguous = resolver.ambiguous.get();
        for (edge, to) in all.edges.iter_mut().zip(resolved) {
            edge.to = to;
        }
        let mut seen = HashSet::new();
        all.edges.retain(|e| seen.insert(e.clone()));
        for (test, targets) in all.tests.iter_mut().zip(targets) {
            test.targets = targets;
        }
        for (function, to) in all.functions.iter_mut().zip(forwards) {
            function.forwards_to = to;
        }
        if ambiguous > 0 {
            notices.push(format!(
                "{ambiguous} edge target(s) matched several symbols and stayed unresolved"
            ));
        }
        let index = Index::new(&all);
        Self {
            all,
            index,
            notices,
        }
    }

    /// Things merging dropped or could not decide, for the user to see.
    pub fn notices(&self) -> &[String] {
        &self.notices
    }

    pub fn file(&self, path: &Path) -> Option<&File> {
        let at = self
            .all
            .files
            .binary_search_by(|f| f.path.as_path().cmp(path))
            .ok()?;
        self.all.files.get(at)
    }

    pub fn symbol(&self, id: &SymbolId) -> Option<&Symbol> {
        self.all.symbols.get(*self.index.symbols.get(id)?)
    }

    /// Symbols declared in `path`, sorted by id.
    pub fn symbols_in<'a>(&'a self, path: &Path) -> impl Iterator<Item = &'a Symbol> + use<'a> {
        let at = self.index.in_file.get(path).map_or(&[][..], Vec::as_slice);
        at.iter().map(|&i| &self.all.symbols[i])
    }

    /// Whether the symbol is test code: declared in a test file, or in a module
    /// that exists to test another (`test_of`), such as an inline `#[cfg(test)]`
    /// module in a production file.
    pub fn in_test(&self, id: &SymbolId) -> bool {
        let in_test_file = self
            .symbol(id)
            .and_then(|s| self.file(&s.file))
            .is_some_and(|f| f.test);
        in_test_file
            || self
                .module(id.module())
                .is_some_and(|m| m.test_of.is_some())
    }

    /// The module with this path.
    pub fn module(&self, path: &str) -> Option<&Module> {
        let at = self
            .all
            .modules
            .binary_search_by(|m| m.path.as_str().cmp(path))
            .ok()?;
        self.all.modules.get(at)
    }

    pub fn function(&self, id: &SymbolId) -> Option<&FunctionSummary> {
        self.all.functions.get(*self.index.functions.get(id)?)
    }

    /// Distinct symbols with a resolved, non-heuristic `calls` edge to `id`,
    /// excluding itself.
    pub fn callers(&self, id: &SymbolId) -> &[SymbolId] {
        self.index.callers.get(id).map_or(&[], Vec::as_slice)
    }

    /// Distinct symbols with a resolved `references` edge to `id`, excluding
    /// itself: every use of `id` as a value rather than a call. Heuristic edges
    /// are included: they say a symbol might be used.
    pub fn references(&self, id: &SymbolId) -> &[SymbolId] {
        self.index.references.get(id).map_or(&[], Vec::as_slice)
    }

    /// Distinct symbols `id` has a resolved, non-heuristic `calls` edge to,
    /// excluding itself.
    pub fn callees(&self, id: &SymbolId) -> &[SymbolId] {
        self.index.callees.get(id).map_or(&[], Vec::as_slice)
    }
}

impl Index {
    fn new(all: &Fragment) -> Self {
        let mut index = Self::default();
        for (at, symbol) in all.symbols.iter().enumerate() {
            index.symbols.insert(symbol.id.clone(), at);
            index
                .in_file
                .entry(symbol.file.clone())
                .or_default()
                .push(at);
        }
        for (at, function) in all.functions.iter().enumerate() {
            index.functions.insert(function.symbol.clone(), at);
        }
        let mut calls = BTreeSet::new();
        let mut references = BTreeSet::new();
        for edge in &all.edges {
            let (Node::Symbol(from), Target::Resolved(Node::Symbol(to))) = (&edge.from, &edge.to)
            else {
                continue;
            };
            match edge.kind {
                EdgeKind::Calls if from != to && edge.resolution != Resolution::Heuristic => {
                    calls.insert((from, to))
                }
                EdgeKind::References if from != to => references.insert((from, to)),
                _ => false,
            };
        }
        for (from, to) in calls {
            index
                .callees
                .entry(from.clone())
                .or_default()
                .push(to.clone());
            index
                .callers
                .entry(to.clone())
                .or_default()
                .push(from.clone());
        }
        for (from, to) in references {
            index
                .references
                .entry(to.clone())
                .or_default()
                .push(from.clone());
        }
        index
    }
}

struct Resolver<'a> {
    symbols: BTreeSet<&'a str>,
    bare: BTreeMap<&'a str, Vec<(&'a SymbolId, SymbolKind)>>,
    modules: BTreeSet<&'a str>,
    ambiguous: Cell<usize>,
}

impl<'a> Resolver<'a> {
    fn new(all: &'a Fragment) -> Self {
        let mut bare: BTreeMap<&str, Vec<(&SymbolId, SymbolKind)>> = BTreeMap::new();
        for symbol in &all.symbols {
            let id = symbol.id.as_str();
            bare.entry(id.split_once('#').map_or(id, |(head, _)| head))
                .or_default()
                .push((&symbol.id, symbol.kind));
        }
        Self {
            symbols: all.symbols.iter().map(|s| s.id.as_str()).collect(),
            bare,
            modules: all.modules.iter().map(|m| m.path.as_str()).collect(),
            ambiguous: Cell::new(0),
        }
    }

    /// A kind-less path naming several symbols resolves only for a call that
    /// matches exactly one function or method; otherwise it stays a path.
    fn resolve(&self, target: &Target, call: bool) -> Target {
        let Target::Path(path) = target else {
            return target.clone();
        };
        let symbol = |id: &str| Target::Resolved(Node::Symbol(SymbolId(id.to_owned())));
        if self.symbols.contains(path.as_str()) {
            return symbol(path);
        }
        if self.modules.contains(path.as_str()) {
            return Target::Resolved(Node::Module(path.clone()));
        }
        let Some(found) = self.bare.get(path.as_str()) else {
            return target.clone();
        };
        if let [(only, _)] = found.as_slice() {
            return symbol(only.as_str());
        }
        if call {
            let callable: Vec<_> = found
                .iter()
                .filter(|(_, kind)| matches!(kind, SymbolKind::Function | SymbolKind::Method))
                .collect();
            if let [(only, _)] = callable.as_slice() {
                return symbol(only.as_str());
            }
        }
        self.ambiguous.set(self.ambiguous.get() + 1);
        target.clone()
    }
}

fn duplicate_notice(sorted: &[Symbol]) -> Option<String> {
    let mut groups: Vec<(&SymbolId, Vec<String>)> = Vec::new();
    for pair in sorted.windows(2) {
        if pair[0].id != pair[1].id || pair[0].file == pair[1].file {
            continue;
        }
        let (a, b) = (
            pair[0].file.display().to_string(),
            pair[1].file.display().to_string(),
        );
        match groups.last_mut() {
            Some((id, files)) if **id == pair[0].id => files.push(b),
            _ => groups.push((&pair[0].id, vec![a, b])),
        }
    }
    if groups.is_empty() {
        return None;
    }
    let examples: Vec<String> = groups
        .iter()
        .take(3)
        .map(|(id, files)| format!("{} ({})", id.as_str(), files.join(", ")))
        .collect();
    Some(format!(
        "{} symbol id(s) are declared in several files and only the first is analyzed: {}",
        groups.len(),
        examples.join("; ")
    ))
}
