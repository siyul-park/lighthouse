//! The one SHA-256 of the workspace: content hashes, fingerprints, record ids
//! and trust bases all hex-encode the same digest.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::canonical;

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
    pub fn hex(self) -> String {
        hex(&self.0.finalize())
    }

    /// The raw 32-byte digest.
    pub fn finish(self) -> [u8; 32] {
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

/// The SHA-256 of the canonical JSON of `value`, in hex.
pub fn digest(value: &Value) -> String {
    sha256(canonical(value))
}

/// The first `bytes` bytes of [`digest`], in hex.
pub fn short_digest(value: &Value, bytes: usize) -> String {
    short(&canonical(value), bytes)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    out
}
