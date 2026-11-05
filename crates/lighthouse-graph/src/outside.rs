//! What a query or a run knows that is not a query: inputs that may be unset,
//! and blobs kept by name.

use std::sync::Arc;

use crate::{
    Db, Durability, Error, Input, Key, Value,
    node::{Locked, NodeId},
    query::encode,
};

/// The value of an input, or `None` when it is not set in this process.
/// The read is recorded either way: the query that read nothing depends
/// on the input being set later.
pub fn try_input<K: Key, V: Value>(
    db: &Db,
    input: &'static Input<K, V>,
    key: &K,
) -> Result<Option<Arc<V>>, Error> {
    let registered = db.shared.register(input);
    let id = NodeId {
        kind: registered.name,
        key: encode(key)?.into(),
    };
    let seen = db.pull(&id, input, false)?;
    if let Some(frame) = &db.frame {
        // An input not set yet may be set with any durability: assume the
        // lowest, so that what read it is not skipped as unaffected.
        let durability = if seen.missing {
            Durability::Volatile
        } else {
            seen.durability
        };
        frame.record(&id, seen.hash, durability);
    }
    match db.pull(&id, input, true) {
        Ok(pulled) => Ok(pulled.value.and_then(|value| value.downcast::<V>().ok())),
        Err(Error::MissingInput(_)) => Ok(None),
        Err(other) => Err(other),
    }
}

/// A blob kept under `name` in the store: what a run knows about the
/// outside world that is no input or query, such as when the files were
/// last walked. It is written at [`Db::commit`] and read back whole.
pub fn state(db: &Db, name: &str) -> Result<Option<Vec<u8>>, Error> {
    if let Some(value) = db.shared.states.locked().get(name) {
        return Ok(Some(value.clone()));
    }
    db.shared.store.state(name)
}

/// Sets the blob [`Db::state`] gives for `name`.
pub fn set_state(db: &mut Db, name: &str, value: Vec<u8>) {
    db.shared.states.locked().insert(name.to_owned(), value);
}
