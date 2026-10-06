mod diagnostic;
mod ucm;

pub use diagnostic::{Diagnostic, Fingerprint, Severity, UnknownSeverity};
pub use ucm::{
    Capability, Edge, EdgeKind, File, Flow, FlowKind, Fragment, FunctionSummary, Module, Node,
    Position, Project, Resolution, Span, Symbol, SymbolId, SymbolKind, Target, TestCase, TestStyle,
    Visibility,
};

/// Rule options as configured in `lighthouse.toml`.
pub type Options = serde_json::Map<String, serde_json::Value>;
