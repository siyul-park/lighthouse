mod frame;
mod serve;
mod wire;

pub use frame::{ErrorObject, FrameError, Id, Message, read_message, write_message};
pub use serve::{Handler, ServeError, serve};
pub use wire::{
    Call, ClientInfo, Comment, Context, Conventions, Edge, EdgeKind, FileInfo, FileRef, Flow,
    FlowKind, Fragment, FunctionSummary, Incomplete, IndexParams, IndexResult, InitializeParams,
    InitializeResult, Methods, Module, Node, Overlay, Position, ProjectRef, ProviderManifest,
    Resolution, Span, Symbol, SymbolKind, SymbolRole, TestCase, TestStyle, Visibility,
};

/// Protocol version spoken by this crate. Peers must agree on it exactly.
pub const VERSION: &str = "0.1";

/// Request: handshake; the first message of a session.
pub const INITIALIZE: &str = "initialize";
/// Request: index the files of one language.
pub const INDEX: &str = "index";
/// Request: stop accepting work; answered, then followed by [`EXIT`].
pub const SHUTDOWN: &str = "shutdown";
/// Notification: end the process; valid only after [`SHUTDOWN`] was answered.
pub const EXIT: &str = "exit";

/// Capability: `calls`, `references`, `implements` and `accesses-private`
/// edges come from the language's own semantic analysis.
pub const SEMANTIC_EDGES: &str = "semantic-edges";
/// Capability: every symbol carries its `extent`, the full declaration range.
pub const EXTENT: &str = "extent";
/// Capability: the provider analyzes the `overlays` of an index request instead
/// of the files on disk. A host declines fixes in files of a provider without it.
pub const OVERLAYS: &str = "overlays";
/// Capability: every reference to a symbol is reported as an edge with a
/// `site`, signatures, fields and receivers included. No bundled provider
/// declares it yet.
pub const COMPLETE_REFERENCES: &str = "complete-references";
/// Capability: edges carry the `site` where the reference occurs.
pub const REFERENCE_SITES: &str = "reference-sites";

/// JSON Schema of every method's params and result, with the wire types under
/// `$defs`.
pub fn schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Methods)).expect("schema serializes")
}
