use lighthouse_model::hash::Hasher;

use super::Digest;

/// Builds a [`Digest`] from parts, the identity of one stored result. Every
/// part is length-prefixed, so that `["ab", "c"]` and `["a", "bc"]` are
/// different keys.
#[derive(Clone, Default)]
pub struct KeyBuilder(Hasher);

impl KeyBuilder {
    /// A key that depends on nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a part.
    pub fn part(&mut self, bytes: impl AsRef<[u8]>) -> &mut Self {
        let bytes = bytes.as_ref();
        self.0.update((bytes.len() as u64).to_le_bytes());
        self.0.update(bytes);
        self
    }

    /// The key of the parts added.
    pub fn finish(self) -> Digest {
        self.0.finish()
    }
}
