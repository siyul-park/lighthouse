//! The one SHA-256 of the workspace: content hashes, fingerprints, record ids
//! and trust bases all hex-encode the same digest.

use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::canonical;

/// A SHA-256 as it is referred to: `sha256:<hex>`.
///
/// Each reader chooses how much it checks: [`parse`](Self::parse) wants the
/// prefix and nothing of the hex, [`lenient`](Self::lenient) takes a bare hex
/// too, and [`is_sha256`](Self::is_sha256) says whether the hex is one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Digest(String);

impl Digest {
    const PREFIX: &'static str = "sha256:";

    /// The digest with the hex `hex`, as it was computed.
    pub fn new(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }

    /// `sha256:<hex>`; none without the prefix. The hex is not checked.
    pub fn parse(text: &str) -> Option<Self> {
        text.strip_prefix(Self::PREFIX).map(Self::new)
    }

    /// `sha256:<hex>` or a bare `<hex>`.
    pub fn lenient(text: &str) -> Self {
        Self::new(text.strip_prefix(Self::PREFIX).unwrap_or(text))
    }

    /// The hex, without the prefix.
    pub fn hex(&self) -> &str {
        &self.0
    }

    /// Whether the hex is that of a SHA-256: 64 lowercase hexadecimal digits.
    pub fn is_sha256(&self) -> bool {
        self.0.len() == 64
            && self
                .0
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}

impl std::fmt::Display for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", Self::PREFIX, self.0)
    }
}

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
