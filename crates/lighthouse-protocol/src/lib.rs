mod frame;
mod serve;
mod wire;

pub use frame::{ErrorObject, FrameError, Id, Message, read_message, write_message};
pub use serve::{Handler, ServeError, serve};
pub use wire::{
    Call, ClientInfo, Comment, Context, Conventions, Edge, EdgeKind, FileInfo, FileRef, Flow,
    FlowKind, Fragment, FunctionSummary, Incomplete, IndexParams, IndexResult, InitializeParams,
    InitializeResult, Language, Methods, Module, Node, Overlay, Position, ProjectRef, Resolution,
    Span, Symbol, SymbolKind, SymbolRole, TestCase, TestStyle, Visibility,
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

/// The only capability defined by version 0.1.
pub const SEMANTIC_EDGES: &str = "semantic-edges";

/// JSON Schema of every method's params and result, with the wire types under
/// `$defs`.
pub fn schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Methods)).expect("schema serializes")
}
