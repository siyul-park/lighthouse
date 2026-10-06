use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Every method with its params and result. Exists to anchor the schema.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Methods {
    pub initialize: Call<InitializeParams, InitializeResult>,
    pub index: Call<IndexParams, IndexResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Call<P, R> {
    pub params: P,
    pub result: R,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// First request, sent by the host. Fields of protocol messages are camelCase;
/// fields of the code model are snake_case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Absolute path of the project root.
    pub root: String,
    pub protocol_version: String,
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Plugin id; must equal the `id` of its `lighthouse-plugin.toml`.
    pub id: String,
    pub version: String,
    /// Must equal the host's protocol version.
    pub protocol_version: String,
    pub languages: Vec<Language>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Language {
    pub id: String,
    /// Project-relative path globs; `*` and `?` never cross `/`, a `**`
    /// component matches any number of directories.
    pub globs: Vec<String>,
    /// Among providers matching a file, the higher priority wins.
    #[serde(default)]
    pub priority: i32,
    /// A fallback language claims a file only when no other language does.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fallback: bool,
    #[serde(default)]
    pub conventions: Conventions,
    /// Known value: `semantic-edges`. Hosts ignore unknown values.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Conventions {
    /// Globs of test files.
    #[serde(default)]
    pub test_globs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectRef {
    /// Absolute path of the project root.
    pub root: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileRef {
    /// Project-relative, `/`-separated.
    pub path: String,
    /// SHA-256 of the file content, lowercase hex.
    pub hash: String,
}

/// Reserved: the content of an unsaved file. Hosts do not send overlays in
/// 0.1; providers read files from disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Overlay {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Context {
    /// Per-language options from `[languages.<id>]`, keyed by language id.
    #[serde(default)]
    pub options: BTreeMap<String, Value>,
    /// Reserved, see [`Overlay`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlays: Option<Vec<Overlay>>,
}

/// Index request. A provider receives every file of one language in one
/// request and decides which packages or modules it must load to analyze
/// them; analysis scope is never narrowed to a subset of the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IndexParams {
    pub project: ProjectRef,
    /// Id of the language the files belong to.
    pub language: String,
    pub files: Vec<FileRef>,
    #[serde(default)]
    pub context: Context,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IndexResult {
    /// One fragment per analyzed file, ordered by path.
    pub fragments: Vec<Fragment>,
    /// Messages that leave the analysis complete.
    #[serde(default)]
    pub notices: Vec<String>,
    /// Files or scopes that could not be analyzed. A file listed here may
    /// still have a fragment with whatever could be recovered.
    #[serde(default)]
    pub incomplete: Vec<Incomplete>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Incomplete {
    /// Project-relative path; absent when the gap is not tied to one file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub reason: String,
}

/// Code-model contribution of one file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Fragment {
    pub file: FileInfo,
    #[serde(default)]
    pub modules: Vec<Module>,
    #[serde(default)]
    pub symbols: Vec<Symbol>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub functions: Vec<FunctionSummary>,
    #[serde(default)]
    pub tests: Vec<TestCase>,
    /// Comments of the file in source order. Added in 0.1 before 1.0; a
    /// plugin that does not know comments omits the field.
    #[serde(default)]
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileInfo {
    pub path: String,
    /// The file is machine-generated.
    #[serde(default, skip_serializing_if = "is_false")]
    pub generated: bool,
}

/// Package, directory or namespace; `path` identifies it project-wide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Module {
    pub path: String,
    /// Name the language gives the module, such as a Go package name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Path of the module whose public surface this one tests from outside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_of: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Visibility {
    Public,
    Private,
    Internal,
}

/// 1-based line and column; the column counts bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Position {
    pub line: u32,
    pub col: u32,
}

/// `end` is exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Span {
    pub start: Position,
    pub end: Position,
}

/// What a declaration of a test file is for. Added in 0.1 before 1.0; a
/// plugin that cannot tell omits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolRole {
    /// Test code that checks on behalf of tests, such as a function that takes
    /// the test framework's handle.
    TestHelper,
    /// Test data and code that builds it: types, constants, variables and
    /// functions that need no handle.
    Fixture,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Symbol {
    /// `module::owner::name#kind`, see docs/plugin-protocol.md.
    pub id: String,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    /// Id of the enclosing type or interface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Project-relative path of the declaring file.
    pub file: String,
    pub span: Span,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    pub name: String,
    /// Set on the declarations of test files whose purpose the language can
    /// tell; test entry points are `test` symbols and carry none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<SymbolRole>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Node {
    Module(String),
    Symbol(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Calls,
    References,
    Imports,
    Contains,
    Implements,
    AccessesPrivate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    /// The target comes from type information.
    Semantic,
    /// The target comes from names alone.
    Syntactic,
    /// A guess: the edge says the source may use the target, never that it
    /// does. Analyzers that count calls or dependencies ignore such edges; a
    /// rule asking whether something might be used may honor them.
    Heuristic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Edge {
    pub kind: EdgeKind,
    pub from: Node,
    /// A module path, a symbol id, or a kind-less symbol id
    /// (`module::owner::name`) that names exactly one symbol.
    pub to: String,
    pub resolution: Resolution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

/// A control-flow construct and the number of enclosing nesting constructs,
/// nested functions included, around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Flow {
    pub kind: FlowKind,
    pub nesting: u32,
    /// `switch`: arms other than the default arm.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub arms: u32,
    /// `logic`: operators in the run.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub operators: u32,
    /// `switch`: every arm, default included, is a single return.
    #[serde(default, skip_serializing_if = "is_false")]
    pub returning: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FunctionSummary {
    pub symbol: String,
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
    /// Reserved: normalized fingerprint for clone detection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clone_fingerprint: Option<String>,
    /// Project types named by the parameters, receiver excluded, as kind-less
    /// symbol ids (`module::name`). Added in 0.1 before 1.0; empty when the
    /// plugin does not resolve parameter types.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_types: Vec<String>,
    /// Checks of a test file's function written out by hand: an `if` that
    /// compares and whose only effect is to fail the test. Added in 0.1
    /// before 1.0; zero when the plugin does not count them.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub manual_assertions: u32,
    /// Target of the call the body consists of when it passes the receiver and
    /// every parameter on, in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwards_to: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TestStyle {
    Table,
    Scenario,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TestCase {
    pub symbol: String,
    /// Depth of nested sub-tests.
    pub nesting: u32,
    pub style: TestStyle,
    /// Symbols the test calls or references, as edge targets.
    pub targets: Vec<String>,
}

/// A comment of the file. Adjacent line comments are one comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Comment {
    pub span: Span,
    /// Source text including the comment markers (`//`, `/* */`, `#`); the
    /// lines of one comment are joined by `\n`.
    pub text: String,
    /// Id of the symbol the comment is the documentation of, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attached_to: Option<String>,
}
