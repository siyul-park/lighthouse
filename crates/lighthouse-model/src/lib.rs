pub mod annotation;
mod diagnostic;
mod document;
mod fix;
pub mod hash;
mod merge;
mod relations;
mod review;
mod scope;
mod text;
mod ucm;

pub use diagnostic::{Diagnostic, Fingerprint, Incomplete, Severity, UnknownSeverity};
pub use document::Document;
pub use fix::{Anchor, EditOp, FixOutcome, Owner, Safety, UnknownSafety};
pub use review::{
    AgentKind, Attribution, Judgment, Label, Suppressed, Suppression, SuppressionKind,
    SuppressionStatus, UnknownTerm,
};
pub use scope::{Applicability, Reach, RunScope, TestScope};
pub use text::{ColumnUnit, LineIndex, byte_position, utf16_position};
pub use ucm::{
    Capability, Comment, Edge, EdgeKind, Event, EventKind, File, Flow, FlowKind, Fragment,
    FunctionSummary, Module, Node, Position, Project, Resolution, Signature, Site, Span, Symbol,
    SymbolId, SymbolKind, SymbolRole, Target, TestCase, TestStyle, TypeRef, Visibility,
};

/// Rule options as configured in `lighthouse.toml`.
pub type Options = serde_json::Map<String, serde_json::Value>;
