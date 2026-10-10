//! Function summaries: the measures of a body, the types of a signature and
//! the events a rule may flag, as the providers give them.

use serde::{Deserialize, Serialize};

use super::{Flow, Span, SymbolId, Target};
use crate::Fingerprint;

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
    /// The types of the parameters and results, receiver excluded.
    #[serde(default)]
    pub signature: Signature,
    /// What the body does that a rule may flag, in source order.
    #[serde(default)]
    pub events: Vec<Event>,
    /// Checks written out by hand in a test file's function: an `if` that
    /// compares and whose only effect is to fail the test.
    #[serde(default)]
    pub manual_assertions: u32,
    /// A method of a trait impl: its signature is not its own to choose.
    #[serde(default)]
    pub implementation: bool,
    /// An associated function without a receiver that returns its owner.
    #[serde(default)]
    pub constructs: bool,
}

/// A type as a signature or a field writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeRef {
    /// The type as written, normalized (see the wire format).
    pub text: String,
    /// The project type it names, as a kind-less symbol id (`module::name`),
    /// pointers, references and aliases stripped; a slice or map names nothing.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Whether that named type is visible outside its module, when known.
    #[serde(default)]
    pub exported: Option<bool>,
}

/// The parameter and result types of a function, receiver excluded, one entry
/// per occurrence in order. A Rust tuple result is one entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub params: Vec<TypeRef>,
    pub results: Vec<TypeRef>,
}

/// What a function body does that a rule may flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    Panic,
    Unwrap,
    ErrorCompare,
    ErrorAssert,
    ErrorfUnwrapped,
}

impl EventKind {
    /// The spelling used in files and expressions.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::Unwrap => "unwrap",
            Self::ErrorCompare => "error-compare",
            Self::ErrorAssert => "error-assert",
            Self::ErrorfUnwrapped => "errorf-unwrapped",
        }
    }
}

/// One body event: its kind, place and a kind-specific detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub kind: EventKind,
    pub span: Span,
    pub detail: Option<String>,
}

/// How a test enumerates its cases: one body over a data table, or separate scenarios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TestStyle {
    Table,
    Scenario,
}
