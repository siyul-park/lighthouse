//! `results.db`: one table of values by key, in WAL mode. Lookups are answered
//! while the run goes, each from a read-only connection of a small pool so that
//! threads do not queue; stores and the use of what was found are written once,
//! in one transaction, at the end.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lighthouse_model::Diagnostic;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{Error, Key};

/// The file the results live in, under the cache directory.
const DB: &str = "results.db";

/// Keeps the directory out of version control whatever the project's own
/// ignore file says.
const IGNORE: &str = "*\n";

/// Bumped whenever the table or what its values mean changes: a database of
/// another version is emptied.
const SCHEMA: i64 = 2;

/// The size, in bytes of the database, past which the least recently used
/// results are pruned: 256 MB. The cap is approximate: it counts the pages of
/// the database file in use, not the write-ahead log.
pub const DEFAULT_LIMIT: u64 = 256 << 20;

/// Pruning goes down to this part of the limit, so that a run that is just over
/// does not prune every time.
const PRUNE_TO: f64 = 0.9;

/// A result that was used is marked used again only when its mark is older
/// than this, so that a run that only reads does not write every key.
const TOUCH_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// What a rule found for one subject and told about it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub diagnostics: Vec<Diagnostic>,
    pub notices: Vec<String>,
}

/// What a run did with the cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub hits: usize,
    pub misses: usize,
    /// Values written.
    pub stored: usize,
    /// Values pruned for the size limit.
    pub pruned: usize,
}

/// A value waiting to be written, as serialized.
struct Pending {
    key: Key,
    value: Vec<u8>,
}

/// An open cache. Lookups and stores may come from several threads.
pub struct Cache {
    path: PathBuf,
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
    used: Mutex<Vec<Key>>,
    fresh: Mutex<Vec<Pending>>,
    limit: u64,
    hits: AtomicUsize,
    misses: AtomicUsize,
}

impl Cache {
    /// Opens the cache in `dir`, creating it. A database that is damaged or
    /// of another version is replaced: it only holds what can be derived
    /// again. Any other failure (a directory that cannot be written, a locked
    /// file) is an error and the run goes on without a cache.
    pub fn open(dir: &Path, limit: u64) -> Result<Self, Error> {
        fs::create_dir_all(dir).map_err(io_error(dir))?;
        let ignore = dir.join(".gitignore");
        if !ignore.exists() {
            fs::write(&ignore, IGNORE).map_err(io_error(&ignore))?;
        }
        let path = dir.join(DB);
        let writer = match connect(&path) {
            Err(Error::Sqlite(e)) if is_damage(&e) => {
                for suffix in ["", "-wal", "-shm"] {
                    let stale = dir.join(format!("{DB}{suffix}"));
                    match fs::remove_file(&stale) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(io_error(&stale)(e)),
                    }
                }
                connect(&path)?
            }
            other => other?,
        };
        Ok(Self {
            path,
            writer: Mutex::new(writer),
            readers: Mutex::default(),
            used: Mutex::default(),
            fresh: Mutex::default(),
            limit,
            hits: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
        })
    }

    /// What was stored under `key`. A value that cannot be read is a miss.
    /// Counting the lookup as a hit or a miss is [`Cache::count`]'s.
    pub fn get<T: DeserializeOwned>(&self, key: &Key) -> Option<T> {
        let found = self.read(key)?;
        let value = serde_json::from_slice(&found.0).ok()?;
        if found.1 + TOUCH_AFTER.as_millis() as i64 <= millis() {
            self.used
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(*key);
        }
        Some(value)
    }

    /// Stores serialized `value` under `key` when the run ends.
    pub fn put_bytes(&self, key: Key, value: Vec<u8>) {
        self.fresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Pending { key, value });
    }

    /// Counts a lookup as answered by the cache, or not.
    pub fn count(&self, hit: bool) {
        let counter = if hit { &self.hits } else { &self.misses };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Stores `value` under `key` when the run ends. A value that cannot be
    /// serialized is not stored.
    pub fn put<T: Serialize>(&self, key: Key, value: &T) {
        let Ok(value) = serde_json::to_vec(value) else {
            return;
        };
        self.fresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Pending { key, value });
    }

    /// Writes what the run stored and used, then prunes down to the limit.
    pub fn finish(&self) -> Result<Stats, Error> {
        let now = millis();
        let fresh = drain(&self.fresh);
        let used = drain(&self.used);
        let mut db = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let tx = db.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO results (key, value, used_at) VALUES (?1, ?2, ?3)",
            )?;
            for stored in &fresh {
                insert.execute(params![stored.key.as_bytes(), stored.value, now])?;
            }
            let mut touch = tx.prepare_cached("UPDATE results SET used_at = ?2 WHERE key = ?1")?;
            for key in &used {
                touch.execute(params![key.as_bytes(), now])?;
            }
        }
        let pruned = prune(&tx, self.limit)?;
        tx.commit()?;
        Ok(Stats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            stored: fresh.len(),
            pruned,
        })
    }

    /// The value and the time it was last used, from a connection of the pool.
    fn read(&self, key: &Key) -> Option<(Vec<u8>, i64)> {
        let pooled = self
            .readers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        let reader = match pooled {
            Some(reader) => reader,
            None => Connection::open_with_flags(
                &self.path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .ok()?,
        };
        let found = reader
            .prepare_cached("SELECT value, used_at FROM results WHERE key = ?1")
            .ok()
            .and_then(|mut query| {
                query
                    .query_row(params![key.as_bytes()], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .optional()
                    .ok()
                    .flatten()
            });
        self.readers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(reader);
        found
    }
}

/// Opens `path` as a results database, in WAL mode, of the current schema.
fn connect(path: &Path) -> Result<Connection, Error> {
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "NORMAL")?;
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != SCHEMA {
        db.execute_batch("DROP TABLE IF EXISTS results")?;
    }
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS results (
             key BLOB PRIMARY KEY,
             value BLOB NOT NULL,
             used_at INTEGER NOT NULL
         ) WITHOUT ROWID;
         CREATE INDEX IF NOT EXISTS results_used ON results (used_at);",
    )?;
    db.pragma_update(None, "user_version", SCHEMA)?;
    // Reads the table once: a damaged file fails here, not in the middle of a run.
    db.query_row("SELECT count(*) FROM results", [], |_| Ok(()))?;
    Ok(db)
}

/// Whether the file is not a usable database, as opposed to being busy or
/// unwritable.
fn is_damage(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(e, _)
            if matches!(e.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    )
}

/// Deletes the least recently used results until what is left is under the
/// limit; returns how many. Counting the stored bytes reads every row, so it is
/// done only when the pages in use say the database may be over.
fn prune(tx: &rusqlite::Transaction, limit: u64) -> Result<usize, Error> {
    let pragma = |name: &str| tx.pragma_query_value(None, name, |row| row.get::<_, i64>(0));
    let in_use = (pragma("page_count")? - pragma("freelist_count")?) * pragma("page_size")?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    if in_use <= limit {
        return Ok(0);
    }
    let total: i64 = tx.query_row(
        "SELECT coalesce(sum(length(key) + length(value)), 0) FROM results",
        [],
        |row| row.get(0),
    )?;
    if total <= limit {
        return Ok(0);
    }
    let target = (limit as f64 * PRUNE_TO) as i64;
    let mut left = total;
    let mut doomed = Vec::new();
    {
        let mut oldest = tx.prepare(
            "SELECT key, length(key) + length(value) FROM results ORDER BY used_at, key",
        )?;
        let mut rows = oldest.query([])?;
        while left > target {
            let Some(row) = rows.next()? else { break };
            left -= row.get::<_, i64>(1)?;
            doomed.push(row.get::<_, Vec<u8>>(0)?);
        }
    }
    let mut delete = tx.prepare_cached("DELETE FROM results WHERE key = ?1")?;
    for key in &doomed {
        delete.execute(params![key])?;
    }
    Ok(doomed.len())
}

/// Empties a list kept behind a lock.
fn drain<T>(list: &Mutex<Vec<T>>) -> Vec<T> {
    std::mem::take(&mut *list.lock().unwrap_or_else(PoisonError::into_inner))
}

fn millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> Error {
    |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}
