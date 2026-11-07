//! The states a node retains, as `graph.db` keeps them: a row each, apart from
//! the node's row, with the value by hash in `blob`. A state that was current
//! on disk is retained with no bytes written.

use rusqlite::{OptionalExtension, Transaction, params};

use crate::{Durability, Error, blob, node::Hash};

/// What a node's retained states change by at a commit.
pub(crate) struct Changes {
    /// States to retain, oldest first.
    pub added: Vec<TraceRow>,
    /// Rows of states that are not retained any more.
    pub lost: Vec<i64>,
}

/// A retained state to store. A `value` of `None` is one the store has.
pub(crate) struct TraceRow {
    pub hash: Hash,
    pub deps: Vec<u8>,
    pub durability: Durability,
    pub value: Option<Vec<u8>>,
}

/// Applies the changes of a node. The hashes of the states it loses are added
/// to `old`.
pub(crate) fn put(
    tx: &Transaction<'_>,
    kind: &str,
    key: &[u8],
    changes: &Changes,
    old: &mut Vec<Vec<u8>>,
) -> Result<(), Error> {
    for row in &changes.lost {
        let gone = tx
            .prepare_cached("DELETE FROM retained WHERE rowid = ?1 RETURNING hash")?
            .query_row([row], |row| row.get(0))
            .optional()?;
        old.extend(gone);
    }
    for trace in &changes.added {
        if let Some(value) = &trace.value {
            blob::insert(tx, trace.hash.as_slice(), value)?;
        }
        tx.prepare_cached(
            "INSERT INTO retained(kind, key, hash, deps, durability) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?
        .execute(params![
            kind,
            key,
            trace.hash.as_slice(),
            trace.deps,
            trace.durability.index() as i64,
        ])?;
    }
    Ok(())
}
