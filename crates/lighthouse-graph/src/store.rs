//! `graph.db`: a table of nodes, a table of the earlier states they retain, the
//! values of both by hash and one row of meta, in WAL mode. A node row is read
//! when the node is pulled or validated, its value apart from the rest; writes
//! of a run go out in one transaction. A value is stored once, whoever has it
//! as current or retained, so retaining a state copies no bytes.

use std::{collections::HashMap, fs, path::Path, sync::Mutex};

use rusqlite::{Connection, ErrorCode, OptionalExtension, params};

use crate::{
    Durability, Error, blob,
    node::{Hash, Locked},
    retained::{self, Changes},
};

/// Bumped whenever a table or what a value means changes: a database of
/// another version is replaced.
const SCHEMA: i64 = 7;

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
    PRIMARY KEY(kind, key)
) WITHOUT ROWID;
CREATE INDEX node_verified ON node(verified_at);
CREATE INDEX node_hash ON node(hash);
CREATE TABLE blob(
    hash BLOB NOT NULL UNIQUE,
    value BLOB NOT NULL
);
CREATE TABLE retained(
    kind TEXT NOT NULL,
    key BLOB NOT NULL,
    hash BLOB NOT NULL,
    deps BLOB NOT NULL,
    durability INTEGER NOT NULL
);
CREATE INDEX retained_node ON retained(kind, key);
CREATE INDEX retained_hash ON retained(hash);
CREATE TRIGGER node_gone AFTER DELETE ON node BEGIN
    DELETE FROM retained WHERE kind = old.kind AND key = old.key;
END;
CREATE TABLE meta(
    id INTEGER PRIMARY KEY CHECK(id = 0),
    revision INTEGER NOT NULL,
    volatile_changed INTEGER NOT NULL,
    stable_changed INTEGER NOT NULL,
    static_changed INTEGER NOT NULL,
    versions BLOB NOT NULL,
    marks BLOB NOT NULL
);";

/// A node to write. A `value` of `None` with `with_value` clears the value.
pub(crate) struct Write {
    pub kind: String,
    pub key: Vec<u8>,
    pub row: Row,
    pub with_value: bool,
    pub value: Option<Vec<u8>>,
    /// What changed in the node's retained states.
    pub traces: Option<Changes>,
}

/// A retained state as stored, without its value.
pub(crate) struct StoredTrace {
    pub row: i64,
    pub hash: Hash,
    pub deps: Vec<u8>,
    pub durability: Durability,
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
    /// The store retains earlier states of the node.
    pub has_traces: bool,
}

/// The state that is not a node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Meta {
    pub revision: u64,
    pub changed: [u64; Durability::COUNT],
    pub versions: Vec<u8>,
    pub marks: Vec<u8>,
}

pub(crate) struct Store {
    conn: Mutex<Connection>,
    /// Every row, read in one scan at the first load: a row is then taken
    /// from memory, not asked of the connection one node at a time.
    rows: Mutex<Option<Rows>>,
}

/// The rows of the nodes not yet touched, by kind and key.
type Rows = HashMap<(String, Vec<u8>), Row>;

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
            rows: Mutex::new(None),
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
                "SELECT revision, volatile_changed, stable_changed, static_changed, versions, marks FROM meta WHERE id = 0",
                [],
                |row| {
                    Ok(Meta {
                        revision: row.get::<_, i64>(0)? as u64,
                        changed: [row.get::<_, i64>(1)? as u64, row.get::<_, i64>(2)? as u64, row.get::<_, i64>(3)? as u64],
                        versions: row.get(4)?,
                        marks: row.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(meta.unwrap_or_default())
    }

    /// The row of a node, without its value; it is taken, so a node is loaded
    /// once.
    pub(crate) fn load(&self, kind: &str, key: &[u8]) -> Result<Option<Row>, Error> {
        let mut rows = self.rows.locked();
        if rows.is_none() {
            *rows = Some(self.scan()?);
        }
        Ok(rows
            .as_mut()
            .and_then(|rows| rows.remove(&(kind.to_owned(), key.to_vec()))))
    }

    /// Every row, without the values.
    fn scan(&self) -> Result<Rows, Error> {
        let conn = self.conn.locked();
        let mut statement = conn.prepare(
            "SELECT kind, key, hash, deps, changed_at, verified_at, durability, input, used_at, EXISTS(SELECT 1 FROM blob WHERE blob.hash = node.hash)
             FROM node",
        )?;
        let mut found = Rows::new();
        let mut cursor = statement.query([])?;
        while let Some(row) = cursor.next()? {
            if let Some(read) = read_row(row, 2)? {
                found.insert((row.get(0)?, row.get(1)?), read);
            }
        }
        drop(cursor);
        drop(statement);
        let mut statement = conn.prepare("SELECT DISTINCT kind, key FROM retained")?;
        let mut cursor = statement.query([])?;
        while let Some(row) = cursor.next()? {
            if let Some(read) = found.get_mut(&(row.get(0)?, row.get(1)?)) {
                read.has_traces = true;
            }
        }
        Ok(found)
    }

    /// The stored value of a node.
    pub(crate) fn value(&self, kind: &str, key: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        let conn = self.conn.locked();
        let mut statement = conn.prepare_cached(
            "SELECT value FROM blob WHERE hash = (SELECT hash FROM node WHERE kind = ?1 AND key = ?2)",
        )?;
        Ok(statement
            .query_row(params![kind, key], |row| row.get(0))
            .optional()?)
    }

    /// The retained states of a node, newest first, without their values.
    pub(crate) fn traces(&self, kind: &str, key: &[u8]) -> Result<Vec<StoredTrace>, Error> {
        let conn = self.conn.locked();
        let mut statement = conn.prepare_cached(
            "SELECT rowid, hash, deps, durability FROM retained WHERE kind = ?1 AND key = ?2 ORDER BY rowid DESC",
        )?;
        let mut cursor = statement.query(params![kind, key])?;
        let mut found = Vec::new();
        while let Some(row) = cursor.next()? {
            let hash: Vec<u8> = row.get(1)?;
            if let Ok(hash) = Hash::try_from(hash.as_slice()) {
                found.push(StoredTrace {
                    row: row.get(0)?,
                    hash,
                    deps: row.get(2)?,
                    durability: Durability::from_index(row.get(3)?),
                });
            }
        }
        Ok(found)
    }

    /// The value with this hash.
    pub(crate) fn blob(&self, hash: &Hash) -> Result<Option<Vec<u8>>, Error> {
        let conn = self.conn.locked();
        let mut statement = conn.prepare_cached("SELECT value FROM blob WHERE hash = ?1")?;
        Ok(statement
            .query_row([hash.as_slice()], |row| row.get(0))
            .optional()?)
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
        let mut old = Vec::new();
        writes
            .iter()
            .try_for_each(|write| put(&tx, write, &mut old))?;
        let aged = tx.execute(
            "DELETE FROM node WHERE verified_at < ?1",
            [forget_before as i64],
        )?;
        put_meta(&tx, meta)?;
        blob::collect(&tx, &old)?;
        if aged > 0 {
            blob::collect_all(&tx)?;
        }
        let pruned = blob::prune(&tx, limit)?;
        tx.commit()?;
        Ok(pruned)
    }
}

fn read_row(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Option<Row>> {
    let hash: Vec<u8> = row.get(at)?;
    let Ok(hash) = Hash::try_from(hash.as_slice()) else {
        return Ok(None);
    };
    Ok(Some(Row {
        hash,
        deps: row.get(at + 1)?,
        changed_at: row.get::<_, i64>(at + 2)? as u64,
        verified_at: row.get::<_, i64>(at + 3)? as u64,
        durability: Durability::from_index(row.get(at + 4)?),
        input: row.get(at + 5)?,
        used_at: row.get::<_, i64>(at + 6)? as u64,
        has_value: row.get(at + 7)?,
        has_traces: false,
    }))
}

/// Writes a node. The hashes of the values it stops having are added to `old`.
fn put(tx: &rusqlite::Transaction<'_>, write: &Write, old: &mut Vec<Vec<u8>>) -> Result<(), Error> {
    if let Some(changes) = &write.traces {
        retained::put(tx, &write.kind, &write.key, changes, old)?;
    }
    if write.with_value {
        old.extend(blob::put(tx, write)?);
    }
    let row = &write.row;
    tx.prepare_cached(
        "INSERT INTO node(kind, key, hash, deps, changed_at, verified_at, durability, input, used_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(kind, key) DO UPDATE SET hash = ?3, deps = ?4, changed_at = ?5, verified_at = ?6,
             durability = ?7, input = ?8, used_at = ?9",
    )?
    .execute(params![
        write.kind,
        write.key,
        row.hash.as_slice(),
        row.deps,
        row.changed_at as i64,
        row.verified_at as i64,
        row.durability.index() as i64,
        row.input,
        row.used_at as i64,
    ])?;
    Ok(())
}

fn put_meta(tx: &rusqlite::Transaction<'_>, meta: &Meta) -> Result<(), Error> {
    let [volatile, stable, fixed] = meta.changed.map(|level| level as i64);
    tx.execute(
        "INSERT INTO meta(id, revision, volatile_changed, stable_changed, static_changed, versions, marks)
         VALUES (0, ?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET revision = ?1, volatile_changed = ?2, stable_changed = ?3,
             static_changed = ?4, versions = ?5, marks = ?6",
        params![meta.revision as i64, volatile, stable, fixed, meta.versions, meta.marks],
    )?;
    Ok(())
}
