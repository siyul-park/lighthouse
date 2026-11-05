//! `graph.db`: one table of nodes and one row of meta, in WAL mode. A node
//! row is read when the node is pulled or validated, its value apart from the
//! rest; writes of a run go out in one transaction.

use std::{fs, path::Path, sync::Mutex};

use rusqlite::{Connection, ErrorCode, OptionalExtension, params};

use crate::{
    Durability, Error,
    node::{Hash, Locked},
};

/// Bumped whenever a table or what a value means changes: a database of
/// another version is replaced.
const SCHEMA: i64 = 3;

/// Pruning goes down to this part of the limit, so that a run that is just
/// over does not prune every time.
const PRUNE_TO: f64 = 0.9;

const TABLES: &str = "
CREATE TABLE node(
    kind TEXT NOT NULL,
    key BLOB NOT NULL,
    hash BLOB NOT NULL,
    deps BLOB NOT NULL,
    changed_at INTEGER NOT NULL,
    verified_at INTEGER NOT NULL,
    durability INTEGER NOT NULL,
    input INTEGER NOT NULL,
    used_at INTEGER NOT NULL,
    value BLOB,
    PRIMARY KEY(kind, key)
) WITHOUT ROWID;
CREATE TABLE meta(
    id INTEGER PRIMARY KEY CHECK(id = 0),
    revision INTEGER NOT NULL,
    volatile_changed INTEGER NOT NULL,
    stable_changed INTEGER NOT NULL,
    static_changed INTEGER NOT NULL,
    versions BLOB NOT NULL
);";

/// A node to write. A `value` of `None` with `with_value` clears the value.
pub(crate) struct Write {
    pub kind: String,
    pub key: Vec<u8>,
    pub row: Row,
    pub with_value: bool,
    pub value: Option<Vec<u8>>,
}

/// What is stored of a node apart from its value.
pub(crate) struct Row {
    pub hash: Hash,
    pub deps: Vec<u8>,
    pub changed_at: u64,
    pub verified_at: u64,
    pub durability: Durability,
    pub input: bool,
    pub used_at: u64,
    pub has_value: bool,
}

/// The state that is not a node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Meta {
    pub revision: u64,
    pub changed: [u64; Durability::COUNT],
    pub versions: Vec<u8>,
}

pub(crate) struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Opens `path`, creating it. A database that is damaged or of another
    /// version is replaced: it only holds what can be derived again.
    pub(crate) fn open(path: &Path) -> Result<Self, Error> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .map_err(|err| Error::Store(rusqlite::Error::ToSqlConversionFailure(err.into())))?;
        }
        match Self::try_open(path) {
            Err(Error::Store(rusqlite::Error::SqliteFailure(failure, _)))
                if matches!(
                    failure.code,
                    ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt
                ) =>
            {
                Self::remove(path);
                Self::try_open(path)
            }
            other => other,
        }
    }

    pub(crate) fn in_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory()?;
        Self::prepare(conn)
    }

    fn try_open(path: &Path) -> Result<Self, Error> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != 0 && version != SCHEMA {
            drop(conn);
            Self::remove(path);
            return Self::try_open(path);
        }
        Self::prepare(conn)
    }

    fn prepare(conn: Connection) -> Result<Self, Error> {
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version == 0 {
            conn.execute_batch(TABLES)?;
            conn.pragma_update(None, "user_version", SCHEMA)?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn remove(path: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            let _ = fs::remove_file(name);
        }
    }

    pub(crate) fn meta(&self) -> Result<Meta, Error> {
        let conn = self.conn.locked();
        let meta = conn
            .query_row(
                "SELECT revision, volatile_changed, stable_changed, static_changed, versions FROM meta WHERE id = 0",
                [],
                |row| {
                    Ok(Meta {
                        revision: row.get::<_, i64>(0)? as u64,
                        changed: [row.get::<_, i64>(1)? as u64, row.get::<_, i64>(2)? as u64, row.get::<_, i64>(3)? as u64],
                        versions: row.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(meta.unwrap_or_default())
    }

    /// The row of a node, without its value.
    pub(crate) fn load(&self, kind: &str, key: &[u8]) -> Result<Option<Row>, Error> {
        let conn = self.conn.locked();
        let mut statement = conn.prepare_cached(
            "SELECT hash, deps, changed_at, verified_at, durability, input, used_at, value IS NOT NULL
             FROM node WHERE kind = ?1 AND key = ?2",
        )?;
        let row = statement
            .query_row(params![kind, key], read_row)
            .optional()?;
        Ok(row.flatten())
    }

    /// The stored value of a node.
    pub(crate) fn value(&self, kind: &str, key: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        let conn = self.conn.locked();
        let mut statement =
            conn.prepare_cached("SELECT value FROM node WHERE kind = ?1 AND key = ?2")?;
        let value = statement
            .query_row(params![kind, key], |row| row.get(0))
            .optional()?;
        Ok(value.flatten())
    }

    /// Writes the nodes and the meta in one transaction, deletes the rows last
    /// verified before revision `forget_before`, then prunes values
    /// down under `limit` bytes, those of lower durability first and the
    /// least recently used first. Returns the nodes whose value was dropped.
    pub(crate) fn commit(
        &self,
        writes: &[Write],
        meta: &Meta,
        limit: u64,
        forget_before: u64,
    ) -> Result<Vec<(String, Vec<u8>)>, Error> {
        let mut conn = self.conn.locked();
        let tx = conn.transaction()?;
        for write in writes {
            put(&tx, write)?;
        }
        tx.execute(
            "DELETE FROM node WHERE verified_at < ?1",
            params![forget_before as i64],
        )?;
        put_meta(&tx, meta)?;
        let pruned = prune(&tx, limit)?;
        tx.commit()?;
        Ok(pruned)
    }
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Row>> {
    let hash: Vec<u8> = row.get(0)?;
    let Ok(hash) = Hash::try_from(hash.as_slice()) else {
        return Ok(None);
    };
    Ok(Some(Row {
        hash,
        deps: row.get(1)?,
        changed_at: row.get::<_, i64>(2)? as u64,
        verified_at: row.get::<_, i64>(3)? as u64,
        durability: Durability::from_index(row.get(4)?),
        input: row.get(5)?,
        used_at: row.get::<_, i64>(6)? as u64,
        has_value: row.get(7)?,
    }))
}

fn put(tx: &rusqlite::Transaction<'_>, write: &Write) -> Result<(), Error> {
    let row = &write.row;
    let common = params![
        write.kind,
        write.key,
        row.hash.as_slice(),
        row.deps,
        row.changed_at as i64,
        row.verified_at as i64,
        row.durability.index() as i64,
        row.input,
        row.used_at as i64,
    ];
    if write.with_value {
        tx.prepare_cached(
            "INSERT INTO node(kind, key, hash, deps, changed_at, verified_at, durability, input, used_at, value)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(kind, key) DO UPDATE SET hash = ?3, deps = ?4, changed_at = ?5, verified_at = ?6,
                 durability = ?7, input = ?8, used_at = ?9, value = ?10",
        )?
        .execute(rusqlite::params_from_iter(
            common.iter().copied().chain([&write.value as &dyn rusqlite::ToSql]),
        ))?;
    } else {
        tx.prepare_cached(
            "UPDATE node SET hash = ?3, deps = ?4, changed_at = ?5, verified_at = ?6, durability = ?7,
                 input = ?8, used_at = ?9 WHERE kind = ?1 AND key = ?2",
        )?
        .execute(common)?;
    }
    Ok(())
}

fn put_meta(tx: &rusqlite::Transaction<'_>, meta: &Meta) -> Result<(), Error> {
    tx.execute(
        "INSERT INTO meta(id, revision, volatile_changed, stable_changed, static_changed, versions)
         VALUES (0, ?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET revision = ?1, volatile_changed = ?2, stable_changed = ?3,
             static_changed = ?4, versions = ?5",
        params![
            meta.revision as i64,
            meta.changed[0] as i64,
            meta.changed[1] as i64,
            meta.changed[2] as i64,
            meta.versions
        ],
    )?;
    Ok(())
}

fn prune(tx: &rusqlite::Transaction<'_>, limit: u64) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let total: i64 = tx.query_row(
        "SELECT COALESCE(SUM(length(value)), 0) FROM node",
        [],
        |row| row.get(0),
    )?;
    if (total as u64) <= limit {
        return Ok(Vec::new());
    }
    let target = (limit as f64 * PRUNE_TO) as i64;
    let mut statement = tx.prepare(
        "SELECT kind, key, length(value) FROM node WHERE value IS NOT NULL ORDER BY durability, used_at",
    )?;
    let mut rows = statement.query([])?;
    let (mut left, mut pruned) = (total, Vec::new());
    while left > target {
        let Some(row) = rows.next()? else { break };
        left -= row.get::<_, i64>(2)?;
        pruned.push((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?));
    }
    drop(rows);
    drop(statement);
    for (kind, key) in &pruned {
        tx.prepare_cached("UPDATE node SET value = NULL WHERE kind = ?1 AND key = ?2")?
            .execute(params![kind, key])?;
    }
    Ok(pruned)
}
