//! `graph.db`: a table of nodes, a table of the earlier states they retain, the
//! values of both by hash and one row of meta, in WAL mode. A node row is read
//! when the node is pulled or validated, its value apart from the rest; writes
//! of a run go out in one transaction. A value is stored once, whoever has it
//! as current or retained, so retaining a state copies no bytes. The file is
//! `auto_vacuum = INCREMENTAL`: pages that rows left are returned to the
//! system a few at a commit, and all of them when the cache is trimmed.

use std::{collections::HashMap, fs, path::Path, sync::Mutex, time::Duration};

use rusqlite::{Connection, ErrorCode, OptionalExtension, params, params_from_iter, types::Value};

use crate::{
    Durability, Error, blob,
    node::{Hash, Locked},
    retained::{self, Changes},
};

/// Bumped whenever a table or what a value means changes: a database of
/// another version is replaced.
const SCHEMA: i64 = 8;

/// `PRAGMA auto_vacuum` value of `INCREMENTAL`.
const INCREMENTAL: i64 = 2;

/// Free pages a commit returns to the system: the cost of a commit stays
/// bounded, the rest goes with the next ones.
const VACUUM_PAGES: u32 = 1024;

/// How long a write waits for another process that has the file.
const BUSY: Duration = Duration::from_secs(5);

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
    cost INTEGER NOT NULL,
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
    /// Microseconds the last run of the node took.
    pub cost: u64,
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
    /// The rows of a kind, read in one scan at the first load of a node of
    /// it: a row is then taken from memory, not asked of the connection one
    /// node at a time, and a kind no node of which is touched is not read.
    rows: Mutex<HashMap<String, Rows>>,
}

/// The rows of the nodes of a kind not yet touched, by key.
type Rows = HashMap<Vec<u8>, Row>;

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
        conn.busy_timeout(BUSY)?;
        // Before the first table: a file made without it cannot take it.
        conn.pragma_update(None, "auto_vacuum", INCREMENTAL)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let mode: i64 = conn.pragma_query_value(None, "auto_vacuum", |row| row.get(0))?;
        if version != 0 && (version != SCHEMA || mode != INCREMENTAL) {
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
            rows: Mutex::default(),
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
        let mut kinds = self.rows.locked();
        if !kinds.contains_key(kind) {
            kinds.insert(kind.to_owned(), self.scan(kind)?);
        }
        Ok(kinds.get_mut(kind).and_then(|rows| rows.remove(key)))
    }

    /// Every row of a kind, without the values.
    fn scan(&self, kind: &str) -> Result<Rows, Error> {
        let conn = self.conn.locked();
        let mut statement = conn.prepare_cached(
            "SELECT key, hash, deps, changed_at, verified_at, durability, input, used_at, cost, EXISTS(SELECT 1 FROM blob WHERE blob.hash = node.hash)
             FROM node WHERE kind = ?1",
        )?;
        let mut found = Rows::new();
        let mut cursor = statement.query([kind])?;
        while let Some(row) = cursor.next()? {
            if let Some(read) = read_row(row, 1)? {
                found.insert(row.get(0)?, read);
            }
        }
        drop(cursor);
        let mut statement =
            conn.prepare_cached("SELECT DISTINCT key FROM retained WHERE kind = ?1")?;
        let mut cursor = statement.query([kind])?;
        while let Some(row) = cursor.next()? {
            if let Some(read) = found.get_mut(&row.get::<_, Vec<u8>>(0)?) {
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

    /// Writes the nodes and the meta in one transaction and deletes the rows
    /// last verified before revision `forget_before`, those of the `owned`
    /// kinds only with the row of their owner, then returns a bounded number
    /// of the free pages to the system.
    pub(crate) fn commit(
        &self,
        writes: &[Write],
        meta: &Meta,
        (forget_before, owned): (u64, &[(&str, &str)]),
    ) -> Result<(), Error> {
        let mut conn = self.conn.locked();
        let tx = conn.transaction()?;
        let mut old = Vec::new();
        writes
            .iter()
            .try_for_each(|write| put(&tx, write, &mut old))?;
        let aged = forget(&tx, forget_before, owned)?;
        put_meta(&tx, meta)?;
        blob::collect(&tx, &old)?;
        if aged > 0 {
            blob::collect_all(&tx)?;
        }
        tx.commit()?;
        vacuum(&conn, VACUUM_PAGES)?;
        Ok(())
    }

    /// Moves the log into the file and empties it: the file the log of a big
    /// commit grew is as small as what it holds, and no page is rewritten
    /// that the log did not hold.
    pub(crate) fn checkpoint(&self) -> Result<(), Error> {
        let conn = self.conn.locked();
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    /// Frees at least `bytes` of stored values, fewer when there are not so
    /// many: the states nodes retain, oldest first, then the values of nodes,
    /// those of lower durability first and the most bytes for the time first.
    /// Every free page goes back to the system and the log is emptied, so the
    /// file is as small as what it holds. Returns the nodes whose value went.
    pub(crate) fn trim(&self, bytes: u64) -> Result<Vec<(String, Vec<u8>)>, Error> {
        let mut conn = self.conn.locked();
        let tx = conn.transaction()?;
        let dropped = blob::free(&tx, bytes)?;
        tx.commit()?;
        vacuum(&conn, 0)?;
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(dropped)
    }
}

/// Returns up to `pages` free pages to the system, all of them for `0`. The
/// pragma frees a page at each step.
fn vacuum(conn: &Connection, pages: u32) -> Result<(), Error> {
    let mut statement = conn.prepare(&format!("PRAGMA incremental_vacuum({pages})"))?;
    let mut steps = statement.query([])?;
    while steps.next()?.is_some() {}
    Ok(())
}

/// Deletes the rows last verified before `before`, apart from those of the
/// kinds that live with another: a row of those goes when no row of its owner
/// is left at its key, which is how a pair is never left with one of them.
/// Returns how many rows went.
fn forget(
    tx: &rusqlite::Transaction<'_>,
    before: u64,
    owned: &[(&str, &str)],
) -> Result<usize, Error> {
    let marks: Vec<String> = (2..owned.len() + 2).map(|at| format!("?{at}")).collect();
    let kinds = std::iter::once(Value::Integer(before as i64)).chain(
        owned
            .iter()
            .map(|(kind, _)| Value::Text((*kind).to_owned())),
    );
    let mut gone = tx.execute(
        &format!(
            "DELETE FROM node WHERE verified_at < ?1 AND kind NOT IN ({})",
            marks.join(", ")
        ),
        params_from_iter(kinds),
    )?;
    for (kind, owner) in owned {
        gone += tx.execute(
            "DELETE FROM node WHERE kind = ?1
             AND NOT EXISTS (SELECT 1 FROM node o WHERE o.kind = ?2 AND o.key = node.key)",
            params![kind, owner],
        )?;
    }
    Ok(gone)
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
        cost: row.get::<_, i64>(at + 7)? as u64,
        has_value: row.get(at + 8)?,
        has_traces: false,
    }))
}

/// Writes a node. The hashes of the values it stops having are added to `old`.
fn put(tx: &rusqlite::Transaction<'_>, write: &Write, old: &mut Vec<Vec<u8>>) -> Result<(), Error> {
    if let Some(changes) = &write.traces {
        retained::put(tx, &write.kind, &write.key, changes, old)?;
    }
    if write.with_value {
        // The value a node keeps under the same hash has an owner still.
        old.extend(blob::put(tx, write)?.filter(|before| before.as_slice() != write.row.hash));
    }
    let row = &write.row;
    tx.prepare_cached(
        "INSERT INTO node(kind, key, hash, deps, changed_at, verified_at, durability, input, used_at, cost)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(kind, key) DO UPDATE SET hash = ?3, deps = ?4, changed_at = ?5, verified_at = ?6,
             durability = ?7, input = ?8, used_at = ?9, cost = ?10",
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
        row.cost as i64,
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
