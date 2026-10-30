//! Every data shape Lighthouse speaks, and how a document is read and stored:
//! the code model (UCM), findings, judgments and records, signals, and the
//! resource envelope of every spec document with its formats, schemas and
//! files. It runs no process, uses no network and knows nothing of the store.

#[macro_use]
mod vocabulary;

mod diagnostic;
mod document;
mod error;
mod fix;
mod format;
mod fs;
mod model_ref;
mod resource;
mod review;
mod schema;
mod scope;
mod signal;
mod text;
mod time;
mod usage;
mod vars;

pub mod annotation;
pub mod glob;
pub mod ucm;

pub use diagnostic::{
    Diagnostic, Fingerprint, Incomplete, Severity, Suppressed, Suppression, SuppressionKind,
    SuppressionStatus, evidence_digest,
};
pub use document::Document;
pub use error::Error;
pub use fix::{Preview, PreviewEdit, PreviewFile, Safety, Unavailable};
pub use format::{Format, documents, to_yaml};
pub use fs::{
    Base, STATE_DIR, file_in, read_optional, write_atomic, write_atomic_guarded, xdg_dir,
    xdg_dir_in, xdg_plugins,
};
pub use model_ref::ModelRef;
pub use resource::{API_VERSION, Metadata, Resource, Spec, is_uid, kind_of, new_uid, resource};
pub use review::{AgentKind, Attribution, Judged, Judgment, Label, Role};
pub use schema::{SCHEMA_URL_BASE, Schema, header, schema, schema_file};
pub use scope::{ExampleScope, Reach, RunScope, TestScope};
pub use signal::{SignalClass, SignalStatus};
pub use text::{
    ColumnUnit, LineIndex, Position, Span, byte_position, eol_of, slashed, squash, utf16_position,
};
pub use time::{Duration, InvalidDuration, now};
pub use usage::Usage;
pub use vars::{replace_vars, var, var_os};
pub use vocabulary::UnknownTerm;

/// Decision options as configured in `lighthouse.toml`.
pub type Options = serde_json::Map<String, serde_json::Value>;

/// Errors cross threads: the MCP server runs operations on a blocking pool.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
