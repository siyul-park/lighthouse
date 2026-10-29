use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Deref,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod applicability;
mod discriminator;
mod edit;
mod function;
mod merge;

pub use applicability::Applicability;
pub use edit::{Anchor, EditOp, FixOutcome, Owner};
pub use function::{Event, EventKind, FunctionSummary, Signature, TestStyle, TypeRef};

use crate::{Document, Span};
use merge::{dedup_edges, resolve_all, sites_of, sorted_union};

/// The fact of a file that lists its regions, see [`File::regions`].
pub const REGIONS: &str = "regions";

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

vocabulary!(Capability, "capability" {
    SemanticEdges => "semantic-edges",
    Extent => "extent",
    ReferenceSites => "reference-sites",
    CompleteReferences => "complete-references",
    Overlays => "overlays",
});

/// A project file; `path` is its project-relative identity and `hash` its content hash.
/// `generated` and `test` are the provider's classification, never inferred here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub path: PathBuf,
    pub lang: String,
    pub hash: String,
    pub generated: bool,
    pub test: bool,
    /// What the provider says of the file besides that, by name: the edition
    /// of the crate a Rust file is in, say. A program names one as
    /// `{file.<name>}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub facts: BTreeMap<String, String>,
}

impl File {
    /// Language whose rules and conventions apply to this file. For embedded
    /// code it is the analyzed language; otherwise it is the host language.
    pub fn analysis_language(&self) -> &str {
        self.facts
            .get("analysisLanguage")
            .map_or(&self.lang, String::as_str)
    }

    /// The languages of the regions the host routed to other providers, each
    /// with whether the region is example code, from the fact the host states
    /// as `language:example` or `language:code`, comma separated.
    pub fn regions(&self) -> impl Iterator<Item = (&str, bool)> {
        self.facts
            .get(REGIONS)
            .map_or("", String::as_str)
            .split(',')
            .filter_map(|region| region.split_once(':'))
            .map(|(language, kind)| (language, kind == "example"))
    }
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
    /// The language of the regions of a host file that declare the module,
    /// stated by the host that routed them; `None` for a module of a file of
    /// its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// The module is declared by example code: what documentation shows, not
    /// part of the project. Its calls and references are sites of use, and
    /// nothing is used, owned or tested because of them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub example: bool,
}

/// Project-stable identity: `module::owner::name#kind`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SymbolId(pub(crate) String);

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

vocabulary!(SymbolKind, "symbol kind" {
    Function => "function",
    Method => "method",
    Type => "type",
    Field => "field",
    Variant => "variant",
    Const => "const",
    Var => "var",
    Interface => "interface",
    Test => "test",
});

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
    /// A field a caller may leave out (an `Option`, a collection, a pointer).
    #[serde(default)]
    pub optional: bool,
    /// The type of a field.
    #[serde(default)]
    pub type_ref: Option<TypeRef>,
}

impl Symbol {
    /// A declaration with nothing but what every one has: no owner, extent,
    /// doc, role or type; set those with struct update.
    pub fn new(
        id: SymbolId,
        kind: SymbolKind,
        visibility: Visibility,
        file: PathBuf,
        span: Span,
        name: impl Into<String>,
    ) -> Self {
        Self {
            id,
            kind,
            visibility,
            owner: None,
            file,
            span,
            extent: None,
            doc: None,
            name: name.into(),
            role: None,
            optional: false,
            type_ref: None,
        }
    }
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

vocabulary!(EdgeKind, "edge kind" {
    Calls => "calls",
    References => "references",
    Imports => "imports",
    Contains => "contains",
    Implements => "implements",
    AccessesPrivate => "accesses-private",
});

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
    /// The file of the fragment of each edge that starts at a module, by
    /// position in `edges`.
    origins: BTreeMap<usize, PathBuf>,
    /// Positions in `edges` of the edges that start at a symbol.
    edges_from: BTreeMap<SymbolId, Vec<usize>>,
    /// The types that implement an interface, by resolved `implements` edges.
    implementers: BTreeMap<SymbolId, Vec<SymbolId>>,
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
        let ambiguous = resolve_all(&mut all);
        if ambiguous > 0 {
            notices.push(format!(
                "{ambiguous} edge target(s) matched several symbols and stayed unresolved"
            ));
        }
        let sites = sites_of(&all, &files);
        let origins = dedup_edges(&mut all, &files);
        let mut index = Index::new(&all);
        index.sites = sites;
        index.origins = origins;
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

    /// Whether the symbol is example code: declared in a module that example
    /// code declares, which documentation shows and the project does not use.
    pub fn in_example(&self, id: &SymbolId) -> bool {
        self.module(id.module()).is_some_and(|m| m.example)
    }

    /// The language the symbol is in: the one its module's host stated, else
    /// the language analyzed in its file; `None` for a file the model does not
    /// know.
    pub fn language_of(&self, symbol: &Symbol) -> Option<&str> {
        self.module(symbol.id.module())
            .and_then(|m| m.language.as_deref())
            .or_else(|| self.file(&symbol.file).map(File::analysis_language))
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
        slice(&self.index.callers, id)
    }

    /// Distinct symbols with a resolved `references` edge to `id`, excluding
    /// itself: every use of `id` as a value rather than a call. Heuristic edges
    /// are included: they say a symbol might be used.
    pub fn references(&self, id: &SymbolId) -> &[SymbolId] {
        slice(&self.index.references, id)
    }

    /// Distinct symbols `id` calls or references through a resolved,
    /// non-heuristic edge, excluding itself: every use `id` makes of another
    /// symbol, whether as a call or as a value.
    pub fn uses(&self, id: &SymbolId) -> &[SymbolId] {
        slice(&self.index.uses, id)
    }

    /// Symbols whose `owner` is `id`: fields, methods, associated items.
    pub fn members(&self, id: &SymbolId) -> &[SymbolId] {
        slice(&self.index.members, id)
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
        slice(&self.index.sites, id)
    }

    /// The file the edge at position `at` of `edges` comes from, when it starts
    /// at a module and its fragment is about one file; an edge that starts at
    /// a symbol comes from the file of the symbol.
    pub fn origin(&self, at: usize) -> Option<&Path> {
        self.index.origins.get(&at).map(PathBuf::as_path)
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

    /// The types that implement the interface `id`, in test files too.
    pub fn implementers(&self, id: &SymbolId) -> &[SymbolId] {
        slice(&self.index.implementers, id)
    }

    /// Distinct symbols `id` has a resolved, non-heuristic `calls` edge to,
    /// excluding itself.
    pub fn callees(&self, id: &SymbolId) -> &[SymbolId] {
        slice(&self.index.callees, id)
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
        for (at, function) in all.functions.iter().enumerate() {
            index.functions.insert(function.symbol.clone(), at);
        }
        index.link_edges(all);
        index
    }

    /// Indexes the edges by source, the implementers of interfaces, and the
    /// calls, uses and references between resolved symbols of project code.
    fn link_edges(&mut self, all: &Fragment) {
        let mut calls = BTreeSet::new();
        let mut references = BTreeSet::new();
        let mut uses = BTreeSet::new();
        let examples: BTreeSet<&str> = all
            .modules
            .iter()
            .filter(|m| m.example)
            .map(|m| m.path.as_str())
            .collect();
        for (at, edge) in all.edges.iter().enumerate() {
            let Node::Symbol(from) = &edge.from else {
                continue;
            };
            self.edges_from.entry(from.clone()).or_default().push(at);
            // An example uses nothing: its edges stay as sites of use, and are
            // not callers, references or implementers of what they name.
            if examples.contains(from.module()) {
                continue;
            }
            let Target::Resolved(Node::Symbol(to)) = &edge.to else {
                continue;
            };
            if edge.kind == EdgeKind::Implements {
                self.implementers
                    .entry(to.clone())
                    .or_default()
                    .push(from.clone());
            }
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

/// The entries of `map` under `id`, none when it has none.
fn slice<'a, T>(map: &'a BTreeMap<SymbolId, Vec<T>>, id: &SymbolId) -> &'a [T] {
    map.get(id).map_or(&[], Vec::as_slice)
}
