//! The vocabulary every other crate speaks: the code model (UCM), findings,
//! judgments and records, and signals. Types and their invariants only; it
//! does no I/O.

#[macro_use]
mod vocabulary;

mod clock;
mod diagnostic;
mod document;
mod fix;
mod model_ref;
mod review;
mod scope;
mod signal;
mod text;
mod usage;

pub mod annotation;
pub mod glob;
pub mod ucm;

pub use clock::now;
pub use diagnostic::{
    Diagnostic, Fingerprint, Incomplete, Severity, Suppressed, Suppression, SuppressionKind,
    SuppressionStatus, evidence_digest,
};
pub use document::Document;
pub use fix::{Preview, PreviewEdit, PreviewFile, Safety, Unavailable};
pub use model_ref::ModelRef;
pub use review::{AgentKind, Attribution, Judged, Judgment, Label, Role};
pub use scope::{ExampleScope, Reach, RunScope, TestScope};
pub use signal::{SignalClass, SignalStatus};
pub use text::{
    ColumnUnit, LineIndex, Position, Span, byte_position, eol_of, squash, utf16_position,
};
pub use usage::Usage;
pub use vocabulary::UnknownTerm;

/// Decision options as configured in `lighthouse.toml`.
pub type Options = serde_json::Map<String, serde_json::Value>;

/// Errors cross threads: the MCP server runs operations on a blocking pool.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
