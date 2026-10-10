use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet, HashSet},
    fmt,
    ops::Deref,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Document, Fingerprint};

/// 1-based line and column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub col: u32,
}

/// Source range within one file; `start` is never after `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: Position,
    pub end: Position,
}

/// Provider feature that rules may require.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    SemanticEdges,
    /// Symbols carry an [`Symbol::extent`]: the full declaration range.
    Extent,
    /// Edges carry the [`Edge::site`] where the reference occurs.
    ReferenceSites,
    /// Every reference to a symbol is an edge with a site: signatures, fields
    /// and receivers included, so a rename cannot miss one.
    CompleteReferences,
    /// The provider analyzes the `overlays` of an index request instead of the
    /// files on disk; a fix needs it to verify an edit before writing it.
    Overlays,
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SemanticEdges => f.write_str("semantic-edges"),
            Self::Extent => f.write_str("extent"),
            Self::ReferenceSites => f.write_str("reference-sites"),
            Self::CompleteReferences => f.write_str("complete-references"),
            Self::Overlays => f.write_str("overlays"),
        }
    }
}

/// A project file; `path` is its project-relative identity and `hash` its content hash.
/// `generated` and `test` are the provider's classification, never inferred here.
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
    /// Builds `module::owner::...::name#kind`; the result always satisfies [`SymbolId::parse`].
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

    /// The id text, `module::owner::name#kind`.
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

/// What a symbol declares; the lowercase name is the `#kind` suffix of its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolKind {
    Function,
    Method,
    Type,
    Field,
    /// A variant of an enum or a union.
    Variant,
    Const,
    Var,
    Interface,
    Test,
}

impl SymbolKind {
    const ALL: [Self; 9] = [
        Self::Function,
        Self::Method,
        Self::Type,
        Self::Field,
        Self::Variant,
        Self::Const,
        Self::Var,
        Self::Interface,
        Self::Test,
    ];

    /// The kind's id suffix, such as `function`; inverse of the ids built by [`SymbolId::new`].
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Type => "type",
            Self::Field => "field",
            Self::Variant => "variant",
            Self::Const => "const",
            Self::Var => "var",
            Self::Interface => "interface",
            Self::Test => "test",
        }
    }
}

/// Who may use a symbol, as the language defines it; `Internal` is
/// visible within a bounded unit such as a crate, but not outside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Visibility {
    Public,
    Private,
    Internal,
}

/// What a declaration of a test file is for, when its language can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolRole {
    /// Test code that checks on behalf of tests, such as a function that takes
    /// the test framework's handle.
    TestHelper,
    /// Test data and code that builds it: types, constants, variables and
    /// functions that need no handle.
    Fixture,
}

/// A named declaration. `id` is project-unique; `owner` is the declaring type
/// or interface of a member, `None` at module level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub id: SymbolId,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    pub owner: Option<SymbolId>,
    pub file: PathBuf,
    pub span: Span,
    /// The whole declaration, from its first leading doc comment, attribute or
    /// annotation to its last token, `None` when the provider does not report
    /// it (capability `extent`). It is what moving or deleting the declaration
    /// takes along.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extent: Option<Span>,
    pub doc: Option<String>,
    pub name: String,
    pub role: Option<SymbolRole>,
}

/// A vertex of the dependency graph: a whole module or one symbol.
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

/// The relation an [`Edge`] states between its endpoints.
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

impl EdgeKind {
    /// The spelling used in files and expressions.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Calls => "calls",
            Self::References => "references",
            Self::Imports => "imports",
            Self::Contains => "contains",
            Self::Implements => "implements",
            Self::AccessesPrivate => "accesses-private",
        }
    }
}

impl std::str::FromStr for EdgeKind {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        [
            Self::Calls,
            Self::References,
            Self::Imports,
            Self::Contains,
            Self::Implements,
            Self::AccessesPrivate,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == text)
        .ok_or(())
    }
}

/// How reliable an [`Edge`]'s target is.
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

/// A directed relation from `from` to `to`; unresolved targets stay
/// [`Target::Path`] after [`Project::merge`] when they are unknown or ambiguous.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Edge {
    pub kind: EdgeKind,
    pub from: Node,
    pub to: Target,
    pub resolution: Resolution,
    /// Where the reference occurs, when the provider reports it (capability
    /// `reference-sites`). The same relation used at several places is one
    /// edge per place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<Span>,
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
    /// A construct with no arms, operators or returning arms; set those directly.
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

/// Size and shape measures of one function or method, keyed by its symbol.
/// Counts are the provider's; nothing here is recomputed from source.
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
    /// Project types named by the parameters, receiver excluded, as kind-less
    /// symbol ids (`module::name`).
    #[serde(default)]
    pub param_types: Vec<String>,
    /// Checks written out by hand in a test file's function: an `if` that
    /// compares and whose only effect is to fail the test.
    #[serde(default)]
    pub manual_assertions: u32,
    /// The function implements a method that a trait or interface declares
    /// elsewhere, so its signature is not its own to choose (a method of a
    /// trait impl in Rust).
    #[serde(default)]
    pub implementation: bool,
    /// The function builds a value of its owner type without taking one: an
    /// associated function without a receiver that returns `Self` or the owner.
    #[serde(default)]
    pub constructs: bool,
}

/// How a test enumerates its cases: one body over a data table, or separate scenarios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TestStyle {
    Table,
    Scenario,
}

/// A test symbol and the symbols it exercises (`targets`); `nesting` is its
/// deepest subtest level, 0 for a flat test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestCase {
    pub symbol: SymbolId,
    pub nesting: u32,
    pub style: TestStyle,
    pub targets: Vec<Target>,
}

/// A comment of a file. Comments are facts about a file's text, so the
/// locator is the `span` and nothing here is specific to code: any provider
/// of a text artifact can state them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub file: PathBuf,
    pub span: Span,
    /// Source text with the comment markers; adjacent line comments form one
    /// comment, their lines joined by `\n`.
    pub text: String,
    /// The symbol this comment is the documentation of, when the provider
    /// knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attached_to: Option<SymbolId>,
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
    #[serde(default)]
    pub comments: Vec<Comment>,
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
    documents: Vec<Document>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Index {
    symbols: BTreeMap<SymbolId, usize>,
    in_file: BTreeMap<PathBuf, Vec<usize>>,
    functions: BTreeMap<SymbolId, usize>,
    callers: BTreeMap<SymbolId, Vec<SymbolId>>,
    callees: BTreeMap<SymbolId, Vec<SymbolId>>,
    references: BTreeMap<SymbolId, Vec<SymbolId>>,
    uses: BTreeMap<SymbolId, Vec<SymbolId>>,
    members: BTreeMap<SymbolId, Vec<SymbolId>>,
    sites: BTreeMap<SymbolId, Vec<Site>>,
    /// Positions in `edges` of the edges that start at a symbol.
    edges_from: BTreeMap<SymbolId, Vec<usize>>,
    /// The interfaces a type implements, by resolved `implements` edges.
    implements: BTreeMap<SymbolId, Vec<SymbolId>>,
    /// `(module, name)` of every method an interface declares.
    interface_methods: BTreeSet<(String, String)>,
}

/// One place that refers to a symbol, from an edge with a `site`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub file: PathBuf,
    pub from: Node,
    pub kind: EdgeKind,
    pub resolution: Resolution,
    pub span: Span,
}

impl Deref for Project {
    type Target = Fragment;

    fn deref(&self) -> &Fragment {
        &self.all
    }
}

impl Project {
    /// Sorts, deduplicates and indexes `parts`, then resolves edge targets;
    /// never fails: what it drops or leaves ambiguous is reported by [`Project::notices`].
    pub fn merge(parts: impl IntoIterator<Item = Fragment>) -> Self {
        let (mut all, mut notices, files) = sorted_union(parts);
        let (ambiguous, sited) = resolve_all(&mut all, &files);
        if ambiguous > 0 {
            notices.push(format!(
                "{ambiguous} edge target(s) matched several symbols and stayed unresolved"
            ));
        }
        let mut index = Index::new(&all);
        index.sites = sites_of(&all, sited);
        Self {
            all,
            index,
            notices,
            documents: Vec::new(),
        }
    }

    /// The same project with these documents, sorted by file and line.
    pub fn with_documents(mut self, mut documents: Vec<Document>) -> Self {
        documents.sort_by(|a, b| (&a.file, a.at, &a.name).cmp(&(&b.file, b.at, &b.name)));
        self.documents = documents;
        self
    }

    /// The documents of the project that are not code, in file order.
    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    /// Things merging dropped or could not decide, for the user to see.
    pub fn notices(&self) -> &[String] {
        &self.notices
    }

    /// The file at this exact project-relative path.
    pub fn file(&self, path: &Path) -> Option<&File> {
        let at = self
            .all
            .files
            .binary_search_by(|f| f.path.as_path().cmp(path))
            .ok()?;
        self.all.files.get(at)
    }

    /// The symbol with this id, if declared.
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

    /// The summary of a function or method symbol; `None` for other kinds.
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

    /// Distinct symbols `id` calls or references through a resolved,
    /// non-heuristic edge, excluding itself: every use `id` makes of another
    /// symbol, whether as a call or as a value.
    pub fn uses(&self, id: &SymbolId) -> &[SymbolId] {
        self.index.uses.get(id).map_or(&[], Vec::as_slice)
    }

    /// Symbols whose `owner` is `id`: fields, methods, associated items.
    pub fn members(&self, id: &SymbolId) -> &[SymbolId] {
        self.index.members.get(id).map_or(&[], Vec::as_slice)
    }

    /// The test case of a test symbol.
    pub fn test(&self, id: &SymbolId) -> Option<&TestCase> {
        let at = self.all.tests.binary_search_by(|t| t.symbol.cmp(id)).ok()?;
        self.all.tests.get(at)
    }

    /// Test cases declared in `module`, ordered by id.
    pub fn tests_in(&self, module: &str) -> &[TestCase] {
        let prefix = format!("{module}::");
        let tests = &self.all.tests;
        let start = tests.partition_point(|t| t.symbol.as_str() < prefix.as_str());
        let len = tests[start..].partition_point(|t| t.symbol.as_str().starts_with(&prefix));
        &tests[start..start + len]
    }

    /// Every place that refers to `id` through an edge that has a site: calls,
    /// references and the other relations alike, in file order. Empty when the
    /// provider reports no reference sites.
    pub fn sites(&self, id: &SymbolId) -> &[Site] {
        self.index.sites.get(id).map_or(&[], Vec::as_slice)
    }

    /// Comments of `path`, in source order.
    pub fn comments_in(&self, path: &Path) -> &[Comment] {
        let comments = &self.all.comments;
        let start = comments.partition_point(|c| c.file.as_path() < path);
        let end = comments.partition_point(|c| c.file.as_path() <= path);
        &comments[start..end]
    }

    /// The edges that start at symbol `id`, in the project's edge order.
    pub fn edges_from(&self, id: &SymbolId) -> impl Iterator<Item = &Edge> + use<'_> {
        self.index
            .edges_from
            .get(id)
            .into_iter()
            .flatten()
            .map(|at| &self.all.edges[*at])
    }

    /// The interfaces the type `id` implements.
    pub fn implements(&self, id: &SymbolId) -> &[SymbolId] {
        self.index.implements.get(id).map_or(&[], Vec::as_slice)
    }

    /// Whether some interface of `module` declares a method called `name`.
    pub fn declared_by_interface(&self, module: &str, name: &str) -> bool {
        self.index
            .interface_methods
            .contains(&(module.to_owned(), name.to_owned()))
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
            if let Some(owner) = &symbol.owner {
                index
                    .members
                    .entry(owner.clone())
                    .or_default()
                    .push(symbol.id.clone());
            }
            index
                .in_file
                .entry(symbol.file.clone())
                .or_default()
                .push(at);
        }
        index.interface_methods = interface_methods(all, &index.symbols);
        index.edges_from = edges_from(all);
        index.implements = implements(all);
        for (at, function) in all.functions.iter().enumerate() {
            index.functions.insert(function.symbol.clone(), at);
        }
        index.link_calls(all);
        index
    }

    /// Indexes the calls, uses and references between resolved symbols.
    fn link_calls(&mut self, all: &Fragment) {
        let mut calls = BTreeSet::new();
        let mut references = BTreeSet::new();
        let mut uses = BTreeSet::new();
        for edge in &all.edges {
            let (Node::Symbol(from), Target::Resolved(Node::Symbol(to))) = (&edge.from, &edge.to)
            else {
                continue;
            };
            let precise = edge.resolution != Resolution::Heuristic;
            if from != to && precise && matches!(edge.kind, EdgeKind::Calls | EdgeKind::References)
            {
                uses.insert((from, to));
            }
            match edge.kind {
                EdgeKind::Calls if from != to && precise => calls.insert((from, to)),
                EdgeKind::References if from != to => references.insert((from, to)),
                _ => false,
            };
        }
        for (from, to) in calls {
            self.callees
                .entry(from.clone())
                .or_default()
                .push(to.clone());
            self.callers
                .entry(to.clone())
                .or_default()
                .push(from.clone());
        }
        for (from, to) in uses {
            self.uses.entry(from.clone()).or_default().push(to.clone());
        }
        for (from, to) in references {
            self.references
                .entry(to.clone())
                .or_default()
                .push(from.clone());
        }
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

/// Resolves edge, test and forwarding targets in place, drops duplicate edges
/// (the same relation at several sites is one edge, which keeps its first
/// site), and returns how many targets stayed ambiguous with every edge that
/// has a site and the file of the fragment it came from.
fn resolve_all(
    all: &mut Fragment,
    files: &[Option<PathBuf>],
) -> (usize, Vec<(Edge, Option<PathBuf>)>) {
    let resolver = Resolver::new(all);
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
    let sited: Vec<(Edge, Option<PathBuf>)> = all
        .edges
        .iter()
        .zip(files)
        .filter(|(e, _)| e.site.is_some())
        .map(|(e, f)| (e.clone(), f.clone()))
        .collect();
    let mut seen = HashSet::new();
    all.edges.retain(|e| {
        let key = Edge {
            site: None,
            ..e.clone()
        };
        seen.insert(key)
    });
    for (test, targets) in all.tests.iter_mut().zip(targets) {
        test.targets = targets;
    }
    for (function, to) in all.functions.iter_mut().zip(forwards) {
        function.forwards_to = to;
    }
    (ambiguous, sited)
}

/// The sited edges that end at a symbol, grouped by target, ordered by file
/// and position.
fn sites_of(all: &Fragment, sited: Vec<(Edge, Option<PathBuf>)>) -> BTreeMap<SymbolId, Vec<Site>> {
    let file_of = |from: &Node| match from {
        Node::Symbol(id) => all
            .symbols
            .binary_search_by(|s| s.id.cmp(id))
            .ok()
            .map(|at| all.symbols[at].file.clone()),
        Node::Module(_) => None,
    };
    let mut sites: BTreeMap<SymbolId, Vec<Site>> = BTreeMap::new();
    for (edge, file) in sited {
        let (Target::Resolved(Node::Symbol(to)), Some(span)) = (&edge.to, edge.site) else {
            continue;
        };
        let Some(file) = file.or_else(|| file_of(&edge.from)) else {
            continue;
        };
        sites.entry(to.clone()).or_default().push(Site {
            file,
            from: edge.from.clone(),
            kind: edge.kind,
            resolution: edge.resolution,
            span,
        });
    }
    for list in sites.values_mut() {
        list.sort_by(|a, b| (&a.file, a.span.start).cmp(&(&b.file, b.span.start)));
        list.dedup();
    }
    sites
}

/// The fragments concatenated, sorted and deduplicated by identity, with a
/// notice when a symbol id is declared in several files.
fn sorted_union(
    parts: impl IntoIterator<Item = Fragment>,
) -> (Fragment, Vec<String>, Vec<Option<PathBuf>>) {
    let mut all = Fragment::default();
    let mut edge_files = Vec::new();
    for part in parts {
        let file = match part.files.as_slice() {
            [only] => Some(only.path.clone()),
            _ => None,
        };
        edge_files.extend(part.edges.iter().map(|_| file.clone()));
        all.files.extend(part.files);
        all.modules.extend(part.modules);
        all.symbols.extend(part.symbols);
        all.edges.extend(part.edges);
        all.functions.extend(part.functions);
        all.tests.extend(part.tests);
        all.comments.extend(part.comments);
    }
    all.files.sort_by(|a, b| a.path.cmp(&b.path));
    all.files.dedup_by(|a, b| a.path == b.path);
    all.modules.sort_by(|a, b| a.path.cmp(&b.path));
    all.modules.dedup_by(|a, b| a.path == b.path);
    all.symbols.sort_by(|a, b| a.id.cmp(&b.id));
    let notices = duplicate_notice(&all.symbols).into_iter().collect();
    all.symbols.dedup_by(|a, b| a.id == b.id);
    all.functions.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    all.functions.dedup_by(|a, b| a.symbol == b.symbol);
    all.tests.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    all.tests.dedup_by(|a, b| a.symbol == b.symbol);
    all.comments
        .sort_by(|a, b| (&a.file, a.span.start).cmp(&(&b.file, b.span.start)));
    all.comments.dedup();
    (all, notices, edge_files)
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

/// `(module, name)` of every method an interface declares.
fn interface_methods(
    all: &Fragment,
    positions: &BTreeMap<SymbolId, usize>,
) -> BTreeSet<(String, String)> {
    let declared_by_interface = |symbol: &Symbol| {
        symbol.kind == SymbolKind::Method
            && symbol
                .owner
                .as_ref()
                .and_then(|o| positions.get(o))
                .is_some_and(|at| all.symbols[*at].kind == SymbolKind::Interface)
    };
    all.symbols
        .iter()
        .filter(|s| declared_by_interface(s))
        .map(|s| (s.id.module().to_owned(), s.name.clone()))
        .collect()
}

/// Positions in `all.edges` of the edges that start at each symbol.
fn edges_from(all: &Fragment) -> BTreeMap<SymbolId, Vec<usize>> {
    let mut by_source: BTreeMap<SymbolId, Vec<usize>> = BTreeMap::new();
    for (at, edge) in all.edges.iter().enumerate() {
        if let Node::Symbol(from) = &edge.from {
            by_source.entry(from.clone()).or_default().push(at);
        }
    }
    by_source
}

/// The interfaces each type implements, by resolved `implements` edges.
fn implements(all: &Fragment) -> BTreeMap<SymbolId, Vec<SymbolId>> {
    let mut found: BTreeMap<SymbolId, Vec<SymbolId>> = BTreeMap::new();
    for edge in all.edges.iter().filter(|e| e.kind == EdgeKind::Implements) {
        if let (Node::Symbol(ty), Target::Resolved(Node::Symbol(interface))) =
            (&edge.from, &edge.to)
        {
            found.entry(ty.clone()).or_default().push(interface.clone());
        }
    }
    found
}
