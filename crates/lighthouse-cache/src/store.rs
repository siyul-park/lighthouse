//! `results.db`: one table of findings by key, in WAL mode. Lookups are
//! answered while the run goes; stores and the use of what was found are
//! written once, in one transaction, at the end.

use std::{
    fs,
    path::Path,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use lighthouse_model::Diagnostic;
use rusqlite::{Connection, OptionalExtension, params};

use crate::{Error, Key};

/// The file the results live in, under the cache directory.
const DB: &str = "results.db";

/// Keeps the directory out of version control whatever the project's own
/// ignore file says.
const IGNORE: &str = "*\n";

/// The size, in bytes of stored results, past which the least recently used
/// are pruned: 256 MB.
pub const DEFAULT_LIMIT: u64 = 256 << 20;

/// Pruning goes down to this part of the limit, so that a run that is just over
/// does not prune every time.
const PRUNE_TO: f64 = 0.9;

/// What a rule found for one subject and told about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stored {
    pub diagnostics: Vec<Diagnostic>,
    pub notices: Vec<String>,
}

/// What a run did with the cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub hits: usize,
    pub misses: usize,
    /// Results written.
    pub stored: usize,
    /// Results pruned for the size limit.
    pub pruned: usize,
}

/// A result waiting to be written, as serialized.
struct Pending {
    key: Key,
    diagnostics: Vec<u8>,
    notices: Vec<u8>,
}

/// An open cache. Lookups and stores may come from several threads.
pub struct Cache {
    db: Mutex<Connection>,
    used: Mutex<Vec<Key>>,
    fresh: Mutex<Vec<Pending>>,
    limit: u64,
    hits: AtomicUsize,
    misses: AtomicUsize,
}

impl Cache {
    /// Opens the cache in `dir`, creating it. A database that cannot be read
    /// (damaged, or of another version) is replaced: it only holds what can be
    /// derived again.
    pub fn open(dir: &Path, limit: u64) -> Result<Self, Error> {
        fs::create_dir_all(dir).map_err(io_error(dir))?;
        let ignore = dir.join(".gitignore");
        if !ignore.exists() {
            fs::write(&ignore, IGNORE).map_err(io_error(&ignore))?;
        }
        let path = dir.join(DB);
        let db = match connect(&path) {
            Ok(db) => db,
            Err(_) => {
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
        };
        Ok(Self {
            db: Mutex::new(db),
            used: Mutex::default(),
            fresh: Mutex::default(),
            limit,
            hits: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
        })
    }

    /// What was stored under `key`. A result that cannot be read is a miss.
    pub fn get(&self, key: &Key) -> Option<Stored> {
        let found = {
            let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
            let mut query = db
                .prepare_cached("SELECT diagnostics, notices FROM results WHERE key = ?1")
                .ok()?;
            query
                .query_row(params![key.as_bytes()], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .optional()
                .ok()?
        };
        let entry = found.and_then(|(diagnostics, notices)| {
            Some(Stored {
                diagnostics: serde_json::from_slice(&diagnostics).ok()?,
                notices: serde_json::from_slice(&notices).ok()?,
            })
        });
        match entry {
            Some(entry) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                self.used
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(*key);
                Some(entry)
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Stores `entry` under `key` when the run ends. An entry that cannot be
    /// serialized is not stored.
    pub fn put(&self, key: Key, entry: &Stored) {
        let (Ok(diagnostics), Ok(notices)) = (
            serde_json::to_vec(&entry.diagnostics),
            serde_json::to_vec(&entry.notices),
        ) else {
            return;
        };
        self.fresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Pending {
                key,
                diagnostics,
                notices,
            });
    }

    /// Writes what the run stored and used, then prunes down to the limit.
    pub fn finish(&self) -> Result<Stats, Error> {
        let now = millis();
        let fresh = drain(&self.fresh);
        let used: Vec<Key> = drain(&self.used);
        let mut db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        let tx = db.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO results (key, diagnostics, notices, used_at)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for stored in &fresh {
                insert.execute(params![
                    stored.key.as_bytes(),
                    stored.diagnostics,
                    stored.notices,
                    now
                ])?;
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
}

/// Opens `path` as a results database, in WAL mode.
fn connect(path: &Path) -> Result<Connection, Error> {
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "NORMAL")?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS results (
             key BLOB PRIMARY KEY,
             diagnostics BLOB NOT NULL,
             notices BLOB NOT NULL,
             used_at INTEGER NOT NULL
         ) WITHOUT ROWID;
         CREATE INDEX IF NOT EXISTS results_used ON results (used_at);",
    )?;
    // Reads the table once: a damaged file fails here, not in the middle of a run.
    db.query_row("SELECT count(*) FROM results", [], |_| Ok(()))?;
    Ok(db)
}

/// Deletes the least recently used results until what is left is under the
/// limit; returns how many.
fn prune(tx: &rusqlite::Transaction, limit: u64) -> Result<usize, Error> {
    let size = |key: i64, diagnostics: i64, notices: i64| key + diagnostics + notices;
    let total: i64 = tx.query_row(
        "SELECT coalesce(sum(length(key) + length(diagnostics) + length(notices)), 0) FROM results",
        [],
        |row| row.get(0),
    )?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    if total <= limit {
        return Ok(0);
    }
    let target = (limit as f64 * PRUNE_TO) as i64;
    let mut left = total;
    let mut doomed = Vec::new();
    {
        let mut oldest = tx.prepare(
            "SELECT key, length(key), length(diagnostics), length(notices)
             FROM results ORDER BY used_at, key",
        )?;
        let mut rows = oldest.query([])?;
        while left > target {
            let Some(row) = rows.next()? else { break };
            left -= size(row.get(1)?, row.get(2)?, row.get(3)?);
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
