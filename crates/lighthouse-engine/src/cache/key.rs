use std::cell::RefCell;

use lighthouse_model::Document;
use lighthouse_protocol::hash::Hasher;
use serde::Serialize;

use super::Error;

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

/// A SHA-256.
pub type Digest = [u8; 32];

/// The digest of the documents of a project, which no file key covers.
pub fn documents(documents: &[Document]) -> Result<Digest, Error> {
    let mut hasher = Hasher::new();
    for document in documents {
        feed(
            &mut hasher,
            &(
                &document.file,
                document.at,
                &document.kind,
                &document.name,
                &document.uid,
                &document.labels,
            ),
        )?;
    }
    Ok(hasher.finish())
}

/// The hash of the JSON of `value`.
pub(crate) fn digest_of<T: Serialize + ?Sized>(value: &T) -> Result<Digest, Error> {
    let mut hasher = Hasher::new();
    feed(&mut hasher, value)?;
    Ok(hasher.finish())
}

/// Feeds the JSON of `value` to `hasher`.
fn feed<T: Serialize + ?Sized>(hasher: &mut Hasher, value: &T) -> Result<(), Error> {
    thread_local! {
        static JSON: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }
    JSON.with_borrow_mut(|json| {
        json.clear();
        serde_json::to_writer(&mut *json, value)?;
        hasher.update(&*json);
        hasher.update([0]);
        Ok(())
    })
}
