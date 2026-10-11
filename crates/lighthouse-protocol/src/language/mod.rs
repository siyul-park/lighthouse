//! The language-provider protocol: the handshake and index methods, their
//! wire types, the framing, and the loop a provider serves them with.

pub(crate) mod frame;
pub(crate) mod serve;
pub(crate) mod wire;
