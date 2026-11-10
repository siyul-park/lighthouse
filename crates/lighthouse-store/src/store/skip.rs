//! A run that equals the last recorded run changes no finding but the
//! sighting: findings rows change only in [`Store::record`](super::Store::record),
//! so the same run on the same rows is idempotent and need not be read back.

use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, Transaction, params};

use crate::{Error, Run, Unchecked, schema};

const DIGEST_KEY: &str = "run_digest";
const NOW_KEY: &str = "run_now";

/// The run the findings rows were last recorded from.
pub(super) struct LastRun {
    pub digest: Option<String>,
    now: Option<String>,
}

impl LastRun {
    pub fn read(tx: &Transaction) -> Result<Self, Error> {
        let get = |key: &str| -> Result<Option<String>, Error> {
            Ok(tx
                .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                    r.get(0)
                })
                .optional()?)
        };
        Ok(Self {
            digest: get(DIGEST_KEY)?,
            now: get(NOW_KEY)?,
        })
    }

    pub fn write(tx: &Transaction, digest: &str, now: &str) -> Result<(), Error> {
        for (key, value) in [(DIGEST_KEY, digest), (NOW_KEY, now)] {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2) \
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        Ok(())
    }

    /// Refreshes the sighting of the open, active findings the last run saw
    /// (the rows it stamped), and remembers this run as the last.
    pub fn see_again(&self, tx: &Transaction, run: &Run, now: &str) -> Result<(), Error> {
        tx.execute(
            "UPDATE sightings SET last_seen = ?1, last_commit = ?2, last_dirty = ?3 \
             WHERE last_seen = ?4 AND fingerprint IN \
                 (SELECT fingerprint FROM findings WHERE resolved_at IS NULL AND inactive_at IS NULL)",
            params![now, run.commit, run.dirty, self.now],
        )?;
        tx.execute(
            "UPDATE meta SET value = ?1 WHERE key = ?2",
            params![now, NOW_KEY],
        )?;
        Ok(())
    }
}

/// What a run says besides its sighting: the schema it is stored in, the
/// decisions it ran, its scope, what it could not check, and each finding by
/// its observation digest. Order and repeats of the lists carry no meaning.
pub(super) fn run_digest(run: &Run, observations: &[String]) -> String {
    let mut hasher = blake3::Hasher::new();
    let mut part = |text: &str| {
        hasher.update(&(text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    };
    part(&schema::VERSION.to_string());
    let set = |items: &[String]| -> BTreeSet<String> { items.iter().cloned().collect() };
    for list in [&run.ran, &run.reported, &run.active] {
        let items = set(list);
        part(&items.len().to_string());
        items.iter().for_each(|item| part(item));
    }
    match &run.unchecked {
        Unchecked::Nothing => part("nothing"),
        Unchecked::Everything => part("everything"),
        Unchecked::Paths(paths) => {
            part("paths");
            let items = set(paths);
            part(&items.len().to_string());
            items.iter().for_each(|item| part(item));
        }
    }
    let mut pairs: Vec<(&str, &str)> = run
        .observed
        .iter()
        .zip(observations)
        .map(|(o, d)| (o.fingerprint.as_str(), d.as_str()))
        .collect();
    pairs.sort_unstable();
    part(&pairs.len().to_string());
    for (fingerprint, digest) in pairs {
        part(fingerprint);
        part(digest);
    }
    hasher.finalize().to_hex().to_string()
}
