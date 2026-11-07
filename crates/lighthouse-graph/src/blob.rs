//! The values of `graph.db` by hash: the current value of a node and the values
//! of its retained states are rows of one table, a value that several have is
//! stored once, and one that none has is deleted at the commit.

use rusqlite::{OptionalExtension, Transaction, params};

use crate::{Error, store::Write};

/// Pruning goes down to this part of the limit, so that a run that is just
/// over does not prune every time.
const PRUNE_TO: f64 = 0.9;

/// Stores the value of a write, if it has one, and returns the hash the node
/// had before, whose value may have no owner now.
pub(crate) fn put(tx: &Transaction<'_>, write: &Write) -> Result<Option<Vec<u8>>, Error> {
    let before = tx
        .prepare_cached("SELECT hash FROM node WHERE kind = ?1 AND key = ?2")?
        .query_row(params![write.kind, write.key], |row| row.get(0))
        .optional()?;
    if let Some(value) = &write.value {
        insert(tx, write.row.hash.as_slice(), value)?;
    }
    Ok(before)
}

/// Stores a value under its hash, unless it is stored already.
pub(crate) fn insert(tx: &Transaction<'_>, hash: &[u8], value: &[u8]) -> Result<(), Error> {
    tx.prepare_cached("INSERT OR IGNORE INTO blob(hash, value) VALUES (?1, ?2)")?
        .execute(params![hash, value])?;
    Ok(())
}

/// Deletes those of the values that no node and no retained state has.
pub(crate) fn collect(tx: &Transaction<'_>, hashes: &[Vec<u8>]) -> Result<(), Error> {
    let mut statement = tx.prepare_cached(
        "DELETE FROM blob WHERE hash = ?1
         AND NOT EXISTS (SELECT 1 FROM node WHERE node.hash = ?1)
         AND NOT EXISTS (SELECT 1 FROM retained WHERE retained.hash = ?1)",
    )?;
    hashes
        .iter()
        .try_for_each(|hash| statement.execute([hash]).map(drop))?;
    Ok(())
}

/// Deletes every value that no node and no retained state has.
pub(crate) fn collect_all(tx: &Transaction<'_>) -> Result<(), Error> {
    tx.execute(
        "DELETE FROM blob WHERE hash NOT IN (SELECT hash FROM node UNION SELECT hash FROM retained)",
        [],
    )?;
    Ok(())
}

/// Deletes the values of the nodes, those of lower durability first and the
/// least recently used first, until what is stored is under `limit` bytes,
/// and the states they retained. Returns the nodes whose value went.
pub(crate) fn prune(tx: &Transaction<'_>, limit: u64) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let total: i64 = tx.query_row(
        "SELECT COALESCE(SUM(length(value)), 0) FROM blob",
        [],
        |row| row.get(0),
    )?;
    if (total as u64) <= limit {
        return Ok(Vec::new());
    }
    // Each goes while what the ones before it freed is short of the excess.
    let excess = total - (limit as f64 * PRUNE_TO) as i64;
    let mut statement = tx.prepare(
        "SELECT kind, key, hash FROM (
             SELECT n.kind, n.key, n.hash, COALESCE(SUM(length(b.value)) OVER (
                 ORDER BY n.durability, n.used_at ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS freed
             FROM node n JOIN blob b ON b.hash = n.hash)
         WHERE freed < ?1",
    )?;
    let rows = statement.query_map([excess], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get::<_, Vec<u8>>(2)?))
    })?;
    let pruned: Vec<(String, Vec<u8>, Vec<u8>)> = rows.collect::<Result<_, _>>()?;
    for (kind, key, hash) in &pruned {
        tx.prepare_cached("DELETE FROM blob WHERE hash = ?1")?
            .execute([hash])?;
        tx.prepare_cached("DELETE FROM retained WHERE kind = ?1 AND key = ?2")?
            .execute(params![kind, key])?;
    }
    collect_all(tx)?;
    Ok(pruned
        .into_iter()
        .map(|(kind, key, _)| (kind, key))
        .collect())
}
