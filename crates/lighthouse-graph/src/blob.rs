//! The values of `graph.db` by hash: the current value of a node and the values
//! of its retained states are rows of one table, a value that several have is
//! stored once, and one that none has is deleted at the commit.

use rusqlite::{OptionalExtension, Transaction, params};

use crate::{Error, Graph, store::Write};

/// What a row of a node weighs besides its key and its dependencies, as far
/// as the pages of the file are concerned.
const ROW_BYTES: i64 = 64;

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

/// Frees at least `bytes` of the stored values of `graph`, fewer when there are
/// not so many, and returns the free pages of its file to the system: the
/// states nodes retain first, oldest first, then the values of nodes, those of
/// lower durability first and the biggest first. A node whose value
/// went is run again when it is pulled; nothing else changes. With `0` only the
/// free pages go. Call it with no pull running, after a [`Graph::commit`].
pub fn trim(graph: &mut Graph, bytes: u64) -> Result<(), Error> {
    let dropped = graph.shared.store.trim(bytes)?;
    graph.shared.forget_values(&dropped);
    Ok(())
}

/// Frees at least `bytes` of values: those of retained states that no node
/// has as current, oldest first, then those of nodes (and the states they
/// retained), those of lower durability first and the most bytes for the time first.
/// Returns the nodes whose value went.
pub(crate) fn free(tx: &Transaction<'_>, bytes: u64) -> Result<Vec<(String, Vec<u8>)>, Error> {
    if bytes == 0 {
        return Ok(Vec::new());
    }
    let before = stored(tx)?;
    let wanted = i64::try_from(bytes).unwrap_or(i64::MAX);
    retained_values(tx, wanted)?;
    let left = wanted - (before - stored(tx)?);
    if left <= 0 {
        return Ok(Vec::new());
    }
    let mut dropped = node_values(tx, left)?;
    let left = wanted - (before - stored(tx)?);
    if left > 0 {
        dropped.extend(node_rows(tx, left)?);
    }
    Ok(dropped)
}

/// Deletes the rows of nodes that can be computed again, those of lower
/// durability first and the most bytes for the time first, while what the ones
/// before them freed is short of `excess`: the last thing to go, when no value
/// is left to drop. A node without a row is run when it is pulled. Returns the
/// nodes.
fn node_rows(tx: &Transaction<'_>, excess: i64) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let mut statement = tx.prepare(
        "SELECT kind, key FROM (
             SELECT kind, key, COALESCE(SUM(length(key) + length(deps) + ?2) OVER (
                 ORDER BY durability, (length(key) + length(deps) + ?2) * 1.0 / (cost + 1) DESC, used_at ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS freed
             FROM node WHERE input = 0)
         WHERE freed < ?1",
    )?;
    let rows = statement.query_map(params![excess, ROW_BYTES], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    let doomed: Vec<(String, Vec<u8>)> = rows.collect::<Result<_, _>>()?;
    for (kind, key) in &doomed {
        tx.prepare_cached("DELETE FROM node WHERE kind = ?1 AND key = ?2")?
            .execute(params![kind, key])?;
    }
    collect_all(tx)?;
    Ok(doomed)
}

/// The bytes of the values stored.
fn stored(tx: &Transaction<'_>) -> Result<i64, Error> {
    Ok(tx.query_row(
        "SELECT COALESCE(SUM(length(value)), 0) FROM blob",
        [],
        |row| row.get(0),
    )?)
}

/// Deletes retained states, oldest first, while what the ones before them
/// freed is short of `excess`; states whose value a node has free nothing and
/// stay.
fn retained_values(tx: &Transaction<'_>, excess: i64) -> Result<(), Error> {
    tx.execute(
        "DELETE FROM retained WHERE rowid IN (
             SELECT id FROM (
                 SELECT r.rowid AS id, COALESCE(SUM(length(b.value)) OVER (
                     ORDER BY r.rowid ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS freed
                 FROM retained r JOIN blob b ON b.hash = r.hash
                 WHERE NOT EXISTS (SELECT 1 FROM node n WHERE n.hash = r.hash))
             WHERE freed < ?1)",
        [excess],
    )?;
    collect_all(tx)
}

/// Deletes the values of nodes that can be computed again (not inputs, whose
/// value is all there is of it), and the states they retained, while what the
/// ones before them freed is short of `excess`. A value an input has too
/// stays. Returns the nodes.
///
/// Of one durability the values that free the most bytes for the time the
/// node took to run go first: what is big and quick to make, not a finding,
/// which is a few bytes and the time of a rule. When a node was used last
/// cannot tell them apart: it is stored once a day at most.
fn node_values(tx: &Transaction<'_>, excess: i64) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let mut statement = tx.prepare(
        "SELECT kind, key, hash FROM (
             SELECT n.kind, n.key, n.hash, COALESCE(SUM(length(b.value)) OVER (
                 ORDER BY n.durability, length(b.value) * 1.0 / (n.cost + 1) DESC, n.used_at ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS freed
             FROM node n JOIN blob b ON b.hash = n.hash WHERE n.input = 0)
         WHERE freed < ?1",
    )?;
    let rows = statement.query_map([excess], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get::<_, Vec<u8>>(2)?))
    })?;
    let pruned: Vec<(String, Vec<u8>, Vec<u8>)> = rows.collect::<Result<_, _>>()?;
    for (kind, key, hash) in &pruned {
        tx.prepare_cached(
            "DELETE FROM blob WHERE hash = ?1
             AND NOT EXISTS (SELECT 1 FROM node WHERE input = 1 AND node.hash = ?1)",
        )?
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
