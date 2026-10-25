//! The language-provider protocol: the handshake and index methods, their
//! wire types, the framing, and the loop a provider serves them with.

pub(crate) mod frame;
pub mod selector;
pub(crate) mod serve;
mod source;
pub use source::coalesce_text_map;
pub(crate) mod wire;
