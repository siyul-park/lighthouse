//! `results.db`: one table of what models answered, by key, in WAL mode,
//! `auto_vacuum = INCREMENTAL`. Lookups are answered
//! while the run goes, each from a read-only connection of a small pool so that
//! threads do not queue; stores and the use of what was found are written once,
//! in one transaction, at the end.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

use super::{Digest, Error};

/// The file the results live in, under the cache directory.
const DB: &str = "results.db";

/// Keeps the directory out of version control whatever the project's own
/// ignore file says.
const IGNORE: &str = "*\n";

/// Bumped whenever the table or what its values mean changes: a database of
/// another version is emptied.
const SCHEMA: i64 = 3;

/// The size, in bytes, the cache directory is kept under, the databases and
/// what plugins keep together: 256 MB. See [`fit`](super::fit).
pub const DEFAULT_LIMIT: u64 = 256 << 20;

/// `PRAGMA auto_vacuum` value of `INCREMENTAL`.
const INCREMENTAL: i64 = 2;

/// Free pages a write returns to the system: the cost of a run stays bounded,
/// the rest goes with the next ones.
const VACUUM_PAGES: u32 = 1024;

/// A result that was used is marked used again only when its mark is older
/// than this, so that a run that only reads does not write every key.
const TOUCH_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// What a run did with the cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Values written.
    pub stored: usize,
}

/// An open cache. Lookups and stores may come from several threads.
pub struct Cache {
    path: PathBuf,
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
    used: Mutex<Vec<Digest>>,
    fresh: Mutex<Vec<Write>>,
}

impl Cache {
    /// Opens the cache in `dir`, creating it. A database that is damaged or
    /// of another version is replaced: it only holds what can be derived
    /// again. Any other failure (a directory that cannot be written, a locked
    /// file) is an error and the run goes on without a cache.
    pub fn open(dir: &Path) -> Result<Self, Error> {
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
        })
    }

    /// What was stored under `key`. A value that cannot be read is a miss.
    /// Counting the lookup as a hit or a miss is [`Cache::count`]'s.
    pub fn get<T: DeserializeOwned>(&self, key: &Digest) -> Option<T> {
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
    pub fn put_bytes(&self, key: Digest, value: Vec<u8>) {
        self.fresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Write { key, value });
    }

    /// Stores `value` under `key` when the run ends. A value that cannot be
    /// serialized is not stored.
    pub fn put<T: Serialize>(&self, key: Digest, value: &T) {
        if let Ok(value) = serde_json::to_vec(value) {
            self.put_bytes(key, value);
        }
    }

    /// Writes what the run stored and used.
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
                insert.execute(params![stored.key, stored.value, now])?;
            }
            let mut touch = tx.prepare_cached("UPDATE results SET used_at = ?2 WHERE key = ?1")?;
            for key in &used {
                touch.execute(params![key, now])?;
            }
        }
        tx.commit()?;
        vacuum(&db, VACUUM_PAGES)?;
        Ok(Stats {
            stored: fresh.len(),
        })
    }

    /// Deletes the least recently used results until `bytes` are freed, or
    /// none are left, and returns every free page to the system and empties
    /// the log: the file is as small as what it holds.
    pub fn trim(&self, bytes: u64) -> Result<(), Error> {
        let mut db = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let tx = db.transaction()?;
        prune(&tx, bytes)?;
        tx.commit()?;
        vacuum(&db, 0)?;
        db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    /// The value and the time it was last used, from a connection of the pool.
    fn read(&self, key: &Digest) -> Option<(Vec<u8>, i64)> {
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
                    .query_row(params![key], |row| Ok((row.get(0)?, row.get(1)?)))
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

/// A value waiting to be written, as serialized.
struct Write {
    key: Digest,
    value: Vec<u8>,
}

/// Returns up to `pages` free pages to the system, all of them for `0`. The
/// pragma frees a page at each step.
fn vacuum(db: &Connection, pages: u32) -> Result<(), Error> {
    let mut statement = db.prepare(&format!("PRAGMA incremental_vacuum({pages})"))?;
    let mut steps = statement.query([])?;
    while steps.next()?.is_some() {}
    Ok(())
}

/// Opens `path` as a results database, in WAL mode, of the current schema.
fn connect(path: &Path) -> Result<Connection, Error> {
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(5))?;
    // Before the first table: a file made without it takes it by a VACUUM.
    db.pragma_update(None, "auto_vacuum", INCREMENTAL)?;
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "NORMAL")?;
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != SCHEMA {
        db.execute_batch("DROP TABLE IF EXISTS results")?;
    }
    let mode: i64 = db.pragma_query_value(None, "auto_vacuum", |row| row.get(0))?;
    if mode != INCREMENTAL {
        db.execute_batch("VACUUM")?;
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
    // Reads a page of the table: a damaged file fails here, not in the middle
    // of a run. Damage further in is a read that fails, which is a miss.
    db.prepare("SELECT 1 FROM results LIMIT 1")?.exists([])?;
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

/// Deletes the least recently used results until `bytes` of them are gone,
/// or none are left.
fn prune(tx: &rusqlite::Transaction, bytes: u64) -> Result<(), Error> {
    let wanted = i64::try_from(bytes).unwrap_or(i64::MAX);
    let mut freed = 0;
    let mut doomed = Vec::new();
    {
        let mut oldest = tx.prepare(
            "SELECT key, length(key) + length(value) FROM results ORDER BY used_at, key",
        )?;
        let mut rows = oldest.query([])?;
        while freed < wanted {
            let Some(row) = rows.next()? else { break };
            freed += row.get::<_, i64>(1)?;
            doomed.push(row.get::<_, Vec<u8>>(0)?);
        }
    }
    let mut delete = tx.prepare_cached("DELETE FROM results WHERE key = ?1")?;
    for key in &doomed {
        delete.execute(params![key])?;
    }
    Ok(())
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
