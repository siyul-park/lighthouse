use std::{
    collections::BTreeMap,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod applicability;
mod edit;
mod function;
mod kinds;
mod load_order;
mod markers;
mod merge;
mod model;
mod qualify;
mod slice;
mod table;
mod view;

pub use applicability::Applicability;
pub use edit::{Anchor, EditOp, FixOutcome, Owner};
pub use function::{Event, EventKind, FunctionSummary, Signature, TestStyle, TypeRef};
pub use kinds::{
    CASES, DECLARATIONS, EDGES, HOST, Host, INCOMING, MARKERS, MODULE_CASES, ORDERED_BY, PATHS,
    PAYLOAD, PROJECT, Payload, REGIONS_UNIT, RESOLVED, SITES, SLICE, SYMBOLS, key,
};
pub use load_order::{LoadNeed, LoadPoint, load_needs};
pub use markers::Markers;
pub use model::{Boxed, Model};

use crate::{Document, Span};
use merge::{dedup_edges, resolve_all, settle, sites_of, sorted_union};
pub use slice::{Edges, Incoming, Resolved, Reverse, Sites, Slice, Tie, collisions, declared};
pub use table::{Declarations, Entry, Table};
pub use view::{Held, View};

/// The fact of a file that lists its regions, see [`File::regions`].
pub const REGIONS: &str = "regions";

/// The fact of a file the host states for a language whose declarations are
/// evaluated in the order they are written, see [`File::is_ordered`].
pub const ORDERED: &str = "ordered";

/// The fact of a file the provider read from its source alone, see
/// [`File::is_syntactic`].
pub const SYNTACTIC: &str = "syntactic";

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

vocabulary!(EdgeKind, "edge kind" {
    Calls => "calls",
    References => "references",
    Imports => "imports",
    Contains => "contains",
    Implements => "implements",
    AccessesPrivate => "accesses-private",
});

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

/// All fragments merged: sorted, deduplicated, edge targets resolved.
///
/// A symbol id declared in several files (build variants, for instance) keeps
/// the first declaration in file order; the rest are dropped and reported by
/// [`Project::notices`], as are targets that stayed ambiguous.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    inner: Arc<Merged>,
    #[serde(skip)]
    documents: Vec<Document>,
}

/// What a merge made, shared by the copies of a project.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Merged {
    all: Fragment,
    index: Index,
    notices: Vec<String>,
}

/// UCM contribution of one indexed file; ids are project-stable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fragment {
    pub files: Vec<File>,
    pub modules: Vec<Module>,
    pub symbols: Vec<Symbol>,
    /// Where each symbol is and what it says, one per symbol.
    #[serde(default)]
    pub places: Vec<Place>,
    pub edges: Vec<Edge>,
    pub functions: Vec<FunctionSummary>,
    pub tests: Vec<TestCase>,
    #[serde(default)]
    pub comments: Vec<Comment>,
}

/// A project file; `path` is its project-relative identity.
/// `generated` and `test` are the provider's classification, never inferred here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub path: PathBuf,
    pub lang: String,
    pub generated: bool,
    pub test: bool,
    /// What the provider says of the file besides that, by name: the edition
    /// of the crate a Rust file is in, say. A program names one as
    /// `{file.<name>}`.
    #[serde(default)]
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

    /// Whether the language of the file evaluates its declarations in the order
    /// they are written, so that a name must be bound above the code that uses
    /// it as the module loads. The host states it from the language manifest.
    pub fn is_ordered(&self) -> bool {
        self.facts.contains_key(ORDERED)
    }

    /// Whether the provider read the file from its source alone, as it does a
    /// test file the build context excludes: its tests count for what their
    /// names say, and no decision judges it.
    pub fn is_syntactic(&self) -> bool {
        self.facts.contains_key(SYNTACTIC)
    }

    /// Whether the file is a host whose regions are all example code.
    pub fn is_example(&self) -> bool {
        let mut regions = self.regions().peekable();
        regions.peek().is_some() && regions.all(|(_, example)| example)
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
    #[serde(default)]
    pub name: Option<String>,
    /// The module whose public surface this one tests from outside.
    #[serde(default)]
    pub test_of: Option<String>,
    /// The language of the regions of a host file that declare the module,
    /// stated by the host that routed them; `None` for a module of a file of
    /// its own.
    #[serde(default)]
    pub language: Option<String>,
    /// The module is declared by example code: what documentation shows, not
    /// part of the project. Its calls and references are sites of use, and
    /// nothing is used, owned or tested because of them.
    #[serde(default)]
    pub example: bool,
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
    pub name: String,
    pub role: Option<SymbolRole>,
    /// A field a caller may leave out (an `Option`, a collection, a pointer).
    #[serde(default)]
    pub optional: bool,
    /// The type of a field.
    #[serde(default)]
    pub type_ref: Option<TypeRef>,
    /// A method of a trait impl: its signature is not its own to choose.
    #[serde(default)]
    pub implementation: bool,
}

/// Where a symbol is and what it says: what moves with every edit above it or
/// inside it, apart from the shape of the declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub symbol: SymbolId,
    pub span: Span,
    /// The whole declaration, from its first leading doc comment, attribute or
    /// annotation to its last token, `None` when the provider does not report
    /// it (capability `extent`). It is what moving or deleting the declaration
    /// takes along.
    #[serde(default)]
    pub extent: Option<Span>,
    pub doc: Option<String>,
}

impl Place {
    /// The place of `symbol` at `span`, with no extent and no doc; set those
    /// with struct update.
    pub fn new(symbol: SymbolId, span: Span) -> Self {
        Self {
            symbol,
            span,
            extent: None,
            doc: None,
        }
    }

    /// The declaration as a whole: its extent where the provider reports one,
    /// else its span.
    pub fn whole(&self) -> Span {
        self.extent.unwrap_or(self.span)
    }
}

impl Symbol {
    /// A declaration with nothing but what every one has: no owner, role or
    /// type; set those with struct update.
    pub fn new(
        id: SymbolId,
        kind: SymbolKind,
        visibility: Visibility,
        file: PathBuf,
        name: impl Into<String>,
    ) -> Self {
        Self {
            id,
            kind,
            visibility,
            owner: None,
            file,
            name: name.into(),
            role: None,
            optional: false,
            type_ref: None,
            implementation: false,
        }
    }
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

/// Who may use a symbol, as the language defines it; `Internal` is
/// visible within a bounded unit such as a crate, but not outside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Visibility {
    Public,
    Private,
    Internal,
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

impl std::borrow::Borrow<str> for SymbolId {
    fn borrow(&self) -> &str {
        &self.0
    }
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
    #[serde(default)]
    pub site: Option<Span>,
    /// When the use is evaluated, see [`When`].
    #[serde(default)]
    pub when: When,
}

/// When the use an [`Edge`] states is evaluated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum When {
    /// When the code that holds the use runs: the body of a function.
    #[default]
    Call,
    /// As the module loads, in the order the file is written: an initializer,
    /// a decorator, a class base or the top-level code of a file.
    Load,
}

impl When {
    /// Whether this is the default.
    pub fn is_call(&self) -> bool {
        matches!(self, Self::Call)
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
    #[serde(default)]
    pub attached_to: Option<SymbolId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Index {
    symbols: BTreeMap<SymbolId, usize>,
    in_file: BTreeMap<PathBuf, Vec<usize>>,
    functions: BTreeMap<SymbolId, usize>,
    places: BTreeMap<SymbolId, usize>,
    /// What the edges say, by the symbol at the end that is asked.
    reverse: Arc<Reverse>,
    /// Where the edges refer to a symbol from.
    sites: Arc<Sites>,
    /// The file of the fragment of each edge that starts at a module, by
    /// position in `edges`.
    origins: BTreeMap<usize, PathBuf>,
    /// Positions in `edges` of the edges that start at a symbol.
    edges_from: BTreeMap<SymbolId, Vec<usize>>,
    /// Positions in `edges` of the edges evaluated as a file loads, by file.
    loads: BTreeMap<PathBuf, Vec<usize>>,
}

/// One place that refers to a symbol, from an edge with a `site`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        &self.inner.all
    }
}

impl Project {
    /// Sorts, deduplicates and indexes `parts`, then resolves edge targets;
    /// never fails: what it drops or leaves ambiguous is reported by [`Project::notices`].
    pub fn merge(parts: impl IntoIterator<Item = Fragment>) -> Self {
        let (mut all, mut notices, files) = sorted_union(parts);
        let ambiguous = resolve_all(&mut all);
        if ambiguous > 0 {
            notices.push(ambiguity(ambiguous));
        }
        let table = Table::of(&all);
        let sites = sites_of(&all, &table, &files);
        let origins = dedup_edges(&mut all, &files);
        let reverse = Reverse::link(&table, all.edges.iter());
        Self::indexed(
            all,
            (Arc::new(reverse), Arc::new(Sites(sites))),
            origins,
            notices,
        )
    }

    /// The project of slices resolved against one table, in the order of
    /// their paths, with the edges of all of them: what `merge` makes of the
    /// fragments, without resolving or linking anything again.
    pub fn assemble<'a>(
        resolved: impl IntoIterator<Item = &'a Resolved>,
        (edges, sites): (&Edges, &Arc<Sites>),
    ) -> Self {
        let mut all = Fragment::default();
        let mut ambiguous = 0;
        let mut origins = BTreeMap::new();
        for resolved in resolved {
            ambiguous += resolved.ambiguous;
            let slice = &resolved.slice;
            let part = &slice.fragment;
            all.files.extend(part.files.iter().cloned());
            all.modules.extend(part.modules.iter().cloned());
            all.symbols.extend(part.symbols.iter().cloned());
            all.places.extend(part.places.iter().cloned());
            all.functions.extend(part.functions.iter().cloned());
            all.tests.extend(part.tests.iter().cloned());
            all.comments.extend(part.comments.iter().cloned());
            let dropped = edges
                .dropped
                .get(&slice.path)
                .map_or(&[][..], Vec::as_slice);
            let mut files = slice
                .runs
                .iter()
                .flat_map(|(len, file)| std::iter::repeat_n(file.as_ref(), *len));
            for (at, edge) in part.edges.iter().enumerate() {
                let file = files.next().flatten();
                if dropped.binary_search(&(at as u32)).is_ok() {
                    continue;
                }
                if let (Node::Module(_), Some(file)) = (&edge.from, file) {
                    origins.insert(all.edges.len(), file.clone());
                }
                all.edges.push(edge.clone());
            }
        }
        let mut notices: Vec<String> = settle(&mut all).into_iter().collect();
        if ambiguous > 0 {
            notices.push(ambiguity(ambiguous));
        }
        Self::indexed(
            all,
            (edges.reverse.clone(), sites.clone()),
            origins,
            notices,
        )
    }

    fn indexed(
        all: Fragment,
        (reverse, sites): (Arc<Reverse>, Arc<Sites>),
        origins: BTreeMap<usize, PathBuf>,
        notices: Vec<String>,
    ) -> Self {
        let mut index = Index::new(&all);
        index.reverse = reverse;
        index.sites = sites;
        index.origins = origins;
        index.loads = load_order::index(&all, &index);
        Self {
            inner: Arc::new(Merged {
                all,
                index,
                notices,
            }),
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
        &self.inner.notices
    }

    /// The file the edge at position `at` of `edges` comes from, when it starts
    /// at a module and its fragment is about one file; an edge that starts at
    /// a symbol comes from the file of the symbol.
    pub fn origin(&self, at: usize) -> Option<&Path> {
        self.inner.index.origins.get(&at).map(PathBuf::as_path)
    }
}

impl Index {
    /// The positions of the symbols, functions and edges of `all`.
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
        for (at, place) in all.places.iter().enumerate() {
            index.places.insert(place.symbol.clone(), at);
        }
        for (at, function) in all.functions.iter().enumerate() {
            index.functions.insert(function.symbol.clone(), at);
        }
        for (at, edge) in all.edges.iter().enumerate() {
            if let Node::Symbol(from) = &edge.from {
                index.edges_from.entry(from.clone()).or_default().push(at);
            }
        }
        index
    }
}

fn ambiguity(count: usize) -> String {
    format!("{count} edge target(s) matched several symbols and stayed unresolved")
}
