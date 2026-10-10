pub mod annotation;
mod diagnostic;
mod fix;
pub mod hash;
mod review;
mod text;
mod ucm;

pub use diagnostic::{Diagnostic, Fingerprint, Incomplete, Severity, UnknownSeverity};
pub use fix::{Anchor, EditOp, FixOutcome, Owner, Safety, UnknownSafety};
pub use review::{Label, MismatchedReason, Reason, ReviewerKind, UnknownTerm, Verdict};
pub use text::LineIndex;
pub use ucm::{
    Capability, Comment, Edge, EdgeKind, File, Flow, FlowKind, Fragment, FunctionSummary, Module,
    Node, Position, Project, Resolution, Site, Span, Symbol, SymbolId, SymbolKind, SymbolRole,
    Target, TestCase, TestStyle, Visibility,
};

/// Rule options as configured in `lighthouse.toml`.
pub type Options = serde_json::Map<String, serde_json::Value>;
