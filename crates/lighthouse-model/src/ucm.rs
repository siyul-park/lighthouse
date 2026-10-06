use std::{collections::BTreeSet, fmt, ops::Deref, path::PathBuf};

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
    pub span: Span,
    pub doc: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Node {
    Module(String),
    Symbol(SymbolId),
}

/// Edge target; `Path` is a qualified id or module path resolved by [`Project::merge`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    Resolved(Node),
    Path(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Calls,
    References,
    Imports,
    Contains,
    Implements,
    AccessesPrivate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    Semantic,
    Syntactic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub kind: EdgeKind,
    pub from: Node,
    pub to: Target,
    pub resolution: Resolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSummary {
    pub symbol: SymbolId,
    pub decisions: u32,
    pub max_nesting: u32,
    pub statements: u32,
    pub params: u32,
    pub returns: u32,
    /// Normalized AST/token fingerprint for clone detection.
    pub clone_fingerprint: Option<Fingerprint>,
    pub single_forward_call: bool,
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Project(Fragment);

impl Deref for Project {
    type Target = Fragment;

    fn deref(&self) -> &Fragment {
        &self.0
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
        all.symbols.dedup_by(|a, b| a.id == b.id);

        let symbols: BTreeSet<&str> = all.symbols.iter().map(|s| s.id.as_str()).collect();
        let modules: BTreeSet<&str> = all.modules.iter().map(|m| m.path.as_str()).collect();
        let resolved: Vec<Target> = all
            .edges
            .iter()
            .map(|edge| match &edge.to {
                Target::Path(p) if symbols.contains(p.as_str()) => {
                    Target::Resolved(Node::Symbol(SymbolId(p.clone())))
                }
                Target::Path(p) if modules.contains(p.as_str()) => {
                    Target::Resolved(Node::Module(p.clone()))
                }
                other => other.clone(),
            })
            .collect();
        for (edge, to) in all.edges.iter_mut().zip(resolved) {
            edge.to = to;
        }
        Self(all)
    }
}
