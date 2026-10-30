//! The language-provider protocol: the handshake and index methods, their
//! wire types, the framing, and the loop a provider serves them with.

pub(crate) mod frame;
pub(crate) mod host;
pub mod selector;
pub(crate) mod server;
mod source;
pub use source::{TextOffsets, coalesce_text_map};
pub(crate) mod wire;
