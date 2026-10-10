use lighthouse_model::hash::Hasher;

/// The identity of one stored result: a SHA-256 of everything it depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key([u8; 32]);

impl Key {
    /// The key as stored.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Builds a [`Key`] from parts. Every part is length-prefixed, so that
/// `["ab", "c"]` and `["a", "bc"]` are different keys.
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
    pub fn finish(self) -> Key {
        Key(self.0.finish_bytes())
    }
}
