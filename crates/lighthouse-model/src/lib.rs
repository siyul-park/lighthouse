#[macro_use]
mod vocabulary;

pub mod annotation;
mod canonical;
mod clock;
mod diagnostic;
mod document;
mod fix;
pub mod hash;
mod merge;
mod model_ref;
mod review;
mod scope;
pub mod stats;
mod text;
mod ucm;

pub use canonical::canonical;
pub use clock::now;
pub use diagnostic::{
    Diagnostic, Fingerprint, Incomplete, Severity, Suppressed, Suppression, SuppressionKind,
    SuppressionStatus, evidence_digest,
};
pub use document::Document;
pub use fix::{Anchor, EditOp, FixOutcome, Owner, Preview, PreviewEdit, PreviewFile, Safety};
pub use model_ref::ModelRef;
pub use review::{AgentKind, Attribution, Judgment, Label, Role};
pub use scope::{Applicability, Reach, RunScope, TestScope};
pub use text::{ColumnUnit, LineIndex, byte_position, squash, utf16_position};
pub use ucm::{
    Capability, Comment, Edge, EdgeKind, Event, EventKind, File, Flow, FlowKind, Fragment,
    FunctionSummary, Module, Node, Position, Project, Resolution, Signature, Site, Span, Symbol,
    SymbolId, SymbolKind, SymbolRole, Target, TestCase, TestStyle, TypeRef, Visibility,
};
pub use vocabulary::UnknownTerm;

/// Decision options as configured in `lighthouse.toml`.
pub type Options = serde_json::Map<String, serde_json::Value>;

/// Errors cross threads: the MCP server runs operations on a blocking pool.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
