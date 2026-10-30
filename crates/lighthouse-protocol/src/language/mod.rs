//! The language-provider protocol: the handshake and index methods, their
//! wire types, the framing, and the loop a provider serves them with.

pub(crate) mod frame;
mod host;
mod provider;
pub mod selector;
pub(crate) mod server;
mod source;
pub use source::{TextOffsets, coalesce_text_map};
pub(crate) mod wire;

pub use host::{HostError, HostLanguage, HostParser, HostProvider, HostTree};
pub use provider::{Provider, serve};
