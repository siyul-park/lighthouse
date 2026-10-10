//! The one SHA-256 of the workspace: content hashes, fingerprints, record ids
//! and trust bases all hex-encode the same digest.

use sha2::{Digest, Sha256};

/// A SHA-256 being fed bytes.
#[derive(Clone, Default)]
pub struct Hasher(Sha256);

impl Hasher {
    /// A hasher that has seen nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds `bytes`.
    pub fn update(&mut self, bytes: impl AsRef<[u8]>) {
        self.0.update(bytes);
    }

    /// The digest in hex.
    pub fn finish(self) -> String {
        hex(&self.0.finalize())
    }

    /// The raw 32-byte digest.
    pub fn finish_bytes(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

/// The SHA-256 of `bytes` in hex.
pub fn sha256(bytes: impl AsRef<[u8]>) -> String {
    hex(&Sha256::digest(bytes))
}

/// The first `bytes` bytes of the SHA-256 of `text`, in hex.
pub fn short(text: &str, bytes: usize) -> String {
    hex(&Sha256::digest(text.as_bytes())[..bytes])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
