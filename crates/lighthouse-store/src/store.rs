use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};

use lighthouse_model::{Reason, hash};
use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Row, Transaction, TransactionBehavior, named_params,
    params, types::Type,
};
use serde_json::{Value, json};

use crate::{
    Error, Filter, FindingRecord, FixEvent, LatestReview, NewFix, NewReview, Observed, Rejection,
    Resolved, ReviewEvent, Run, RunSummary, Stamp, Standing, StatusFilter, Unchecked, digest, log,
    log::RewriteSpec, migrations,
};

/// File name of the database inside the store directory.
const FILE: &str = "lighthouse.db";
/// Directory of a project that holds its Lighthouse state.
const DIR: &str = ".lighthouse";
/// How long a writer waits for another process holding the database.
const BUSY: Duration = Duration::from_secs(5);
/// Attempts to switch a database that another process is switching, and the
/// pause between them.
const OPEN_ATTEMPTS: u32 = 100;
const OPEN_PAUSE: Duration = Duration::from_millis(20);
/// SQL for the current UTC time, millisecond resolution.
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";
/// Format version of the feature snapshot of a review: 2 has camelCase keys.
const SNAPSHOT_VERSION: u32 = 2;

const EVENT_COLUMNS: &str = "event_id, fingerprint, rule_id, decision_uid, rule_version, check_revision, decision_hash, \
     catalog_version, lighthouse_version, pattern_fingerprint, verdict, reason_code, reason_text, \
     reviewer_kind, reviewer_id, language, scope, evidence_digest, feature_snapshot, git_commit, \
     timestamp";

/// Most fingerprints an ambiguous prefix error lists.
const MAX_CANDIDATES: usize = 5;

impl Store {
    /// Where a project's cache lives: `<root>/.lighthouse/lighthouse.db`.
    pub fn path_in(root: &Path) -> PathBuf {
        root.join(DIR).join(FILE)
    }

    /// Where a project's decision log lives: `<root>/.lighthouse/decisions.jsonl`.
    pub fn log_path_in(root: &Path) -> PathBuf {
        root.join(DIR).join(log::FILE)
    }

    /// Opens the store of the project at `root`, creating the cache and its
    /// directory, migrating its schema and importing the decision log.
    pub fn open(root: &Path) -> Result<Self, Error> {
        let path = Self::path_in(root);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|source| io(dir, source))?;
        }
        let log = Self::log_path_in(root);
        Self::connect(&path, Some(log))
    }

    /// Opens the store only if the project has a cache or a decision log.
    pub fn open_existing(root: &Path) -> Result<Option<Self>, Error> {
        let exists = Self::path_in(root).is_file() || Self::log_path_in(root).is_file();
        exists.then(|| Self::open(root)).transpose()
    }

    /// A store without a log that lives and dies with this value.
    pub fn open_in_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory()?;
        Self::prepare(conn, None)
    }

    fn connect(path: &Path, log: Option<PathBuf>) -> Result<Self, Error> {
        let attempt = || -> Result<Self, Error> {
            let conn = Connection::open(path)?;
            Self::prepare(conn, log.clone())
        };
        retry_busy(attempt).map_err(|e| diagnose(e, path))
    }

    fn prepare(mut conn: Connection, log: Option<PathBuf>) -> Result<Self, Error> {
        conn.busy_timeout(BUSY)?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        migrations::migrate(&mut conn)?;
        let mut store = Self {
            conn,
            log,
            notices: Vec::new(),
        };
        store.sync()?;
        Ok(store)
    }

    /// What the user should know about how the log was read: records a newer
    /// build wrote that this one skipped.
    pub fn notices(&self) -> &[String] {
        &self.notices
    }

    /// The number of schema migrations the database has applied.
    pub fn schema_version(&self) -> Result<usize, Error> {
        migrations::version(&self.conn)
    }

    /// Records what a run saw. Seen findings are inserted or refreshed (a
    /// resolved one is reopened); findings that were open inside the run's
    /// reported paths and rules and are not seen now are resolved, except
    /// where the run could not check; findings of rules the configuration no
    /// longer enables become inactive.
    pub fn record(&mut self, run: &Run) -> Result<RunSummary, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = timestamp(&tx)?;
        let mut summary = RunSummary::default();
        for observed in &run.observed {
            for legacy in &observed.legacy_fingerprints {
                let rewrite = adopt(&tx, legacy, observed, &mut summary)?;
                if let (Some(rewrite), Some(path)) = (rewrite, &self.log) {
                    log::append_rewrite(path, &rewrite)?;
                }
            }
            upsert(&tx, observed, run, &now, &mut summary)?;
        }
        summary.resolved = resolve_absent(&tx, run, &now)?;
        summary.deactivated = reconcile_inactive(&tx, run, &now)?;
        tx.commit()?;
        Ok(summary)
    }

    /// What the latest rejection does to each finding it applies to: keeps it
    /// out of reports, has expired because the rule or the evidence moved, or
    /// cannot apply because the finding is mechanical (its authored severity, not its
    /// severity, decides).
    pub fn standings(&self) -> Result<BTreeMap<String, Rejection>, Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT fingerprint, standing, reason_code FROM standings")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut found = BTreeMap::new();
        for row in rows {
            let (fingerprint, standing, reason) = row?;
            let standing = Standing::parse(&standing).ok_or_else(|| unknown(&standing))?;
            let reason = reason.parse::<Reason>().map_err(|e| unknown(&e))?;
            found.insert(fingerprint, Rejection { standing, reason });
        }
        Ok(found)
    }

    /// The findings matching `filter`, by path and first sighting.
    pub fn list(&self, filter: &Filter) -> Result<Vec<FindingRecord>, Error> {
        let status = match filter.status {
            StatusFilter::Open => {
                "resolved_at IS NULL AND inactive_at IS NULL AND COALESCE(standing, '') <> 'suppressed'"
            }
            StatusFilter::Suppressed => "standing = 'suppressed'",
            StatusFilter::Narrowing => "standing = 'suppressed' AND narrowing = 1",
            StatusFilter::Inactive => {
                "inactive_at IS NOT NULL AND COALESCE(standing, '') <> 'suppressed'"
            }
            StatusFilter::Resolved => "resolved_at IS NOT NULL",
            StatusFilter::All => "1",
        };
        let sql = format!(
            "SELECT * FROM finding_states WHERE (?1 IS NULL OR rule_id = ?1) AND {status} \
             ORDER BY path, first_seen, fingerprint"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![filter.rule], finding_record)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The finding whose fingerprint is, or uniquely starts with, `fingerprint`.
    pub fn finding(&self, fingerprint: &str) -> Result<FindingRecord, Error> {
        load_finding(&self.conn, fingerprint)
    }

    /// The reviews of a finding, oldest first. The finding need not have been
    /// seen on this machine: a verdict from the decision log is enough.
    pub fn history(&self, fingerprint: &str) -> Result<Vec<ReviewEvent>, Error> {
        let full = expand(&self.conn, fingerprint)?;
        let sql = format!(
            "SELECT {EVENT_COLUMNS} FROM review_events \
             WHERE subject = COALESCE((SELECT current FROM fingerprint_rewrites WHERE legacy = ?1), ?1) \
             ORDER BY timestamp, event_id"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![full], review_event)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Appends a verdict to the decision log, then to the cache. The finding
    /// is read inside the transaction that records the verdict, and its
    /// evidence and the facts of its last sighting are frozen as the
    /// feature snapshot. `stamp` supplies the versions of the finding's rule.
    /// The verdict and reason must belong together, and when `expect_seen` is
    /// set the finding must not have been seen again since.
    pub fn resolve(
        &mut self,
        review: &NewReview,
        stamp: impl FnOnce(&FindingRecord) -> Stamp,
    ) -> Result<Resolved, Error> {
        review.verdict.validate(review.reason)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let finding = load_finding(&tx, &review.fingerprint)?;
        if let Some(expected) = &review.expect_seen
            && *expected != finding.last_seen
        {
            return Err(Error::Changed {
                expected: expected.clone(),
                actual: finding.last_seen,
            });
        }
        let event = event_of(review, &finding, stamp(&finding), timestamp(&tx)?)?;
        if let Some(path) = &self.log {
            log::append(path, &event)?;
        }
        insert_event(&tx, &event)?;
        tx.commit()?;
        Ok(Resolved { event, finding })
    }

    /// Records that a fixer changed the code for a finding. The record is
    /// local: unlike a verdict it is not written to the decision log.
    pub fn record_fix(&mut self, fix: &NewFix) -> Result<FixEvent, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let timestamp = timestamp(&tx)?;
        let files = serde_json::to_string(&fix.files)?;
        static NONCE: AtomicU64 = AtomicU64::new(0);
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        let id = hash::short(
            &format!(
                "{}\0{}\0{}\0{files}\0{timestamp}\0{}\0{nonce}",
                fix.fingerprint,
                fix.fixer,
                fix.description,
                std::process::id()
            ),
            16,
        );
        tx.execute(
            "INSERT OR IGNORE INTO fix_events (event_id, fingerprint, rule_id, fixer, safety, \
                 description, files, git_commit, lighthouse_version, timestamp) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                fix.fingerprint,
                fix.rule_id,
                fix.fixer,
                fix.safety,
                fix.description,
                files,
                fix.commit,
                fix.lighthouse_version,
                timestamp
            ],
        )?;
        tx.commit()?;
        Ok(FixEvent {
            id,
            fingerprint: fix.fingerprint.clone(),
            rule_id: fix.rule_id.clone(),
            fixer: fix.fixer.clone(),
            safety: fix.safety.clone(),
            description: fix.description.clone(),
            files: fix.files.clone(),
            commit: fix.commit.clone(),
            timestamp,
        })
    }

    /// The fixes recorded for a finding, oldest first.
    pub fn fixes(&self, fingerprint: &str) -> Result<Vec<FixEvent>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, fingerprint, rule_id, fixer, safety, description, files, \
                 git_commit, timestamp FROM fix_events WHERE fingerprint = ?1 \
             ORDER BY timestamp, event_id",
        )?;
        let rows = stmt.query_map(params![fingerprint], |row| {
            let files: String = row.get(6)?;
            Ok(FixEvent {
                id: row.get(0)?,
                fingerprint: row.get(1)?,
                rule_id: row.get(2)?,
                fixer: row.get(3)?,
                safety: row.get(4)?,
                description: row.get(5)?,
                files: serde_json::from_str(&files).unwrap_or_default(),
                commit: row.get(7)?,
                timestamp: row.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Deletes findings that are resolved or inactive, were never reviewed and
    /// have been so for at least `older_than_days` days (any time when `None`).
    /// Reviewed findings stay: their verdicts are labels. Returns how many
    /// were deleted.
    pub fn prune(&mut self, older_than_days: Option<u32>) -> Result<usize, Error> {
        let age = match older_than_days {
            Some(days) => format!(
                "AND COALESCE(resolved_at, inactive_at) < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-{days} days')"
            ),
            None => String::new(),
        };
        let sql = format!(
            "DELETE FROM findings WHERE (resolved_at IS NOT NULL OR inactive_at IS NOT NULL) \
             AND NOT EXISTS (SELECT 1 FROM review_events e WHERE e.subject = findings.fingerprint) \
             {age}"
        );
        Ok(self.conn.execute(&sql, [])?)
    }

    /// Teaches the store which uid each decision name belongs to (the names a
    /// decision had before it was renamed included), and gives the findings
    /// and verdicts that have none the uid of their decision. A row whose
    /// name no decision answers to keeps no uid and stays readable by name.
    pub fn identify(&mut self, names: &BTreeMap<String, String>) -> Result<(), Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut put = tx.prepare_cached(
                "INSERT INTO decision_uids (name, uid) VALUES (?1, ?2) \
                 ON CONFLICT (name) DO UPDATE SET uid = excluded.uid",
            )?;
            for (name, uid) in names {
                put.execute(params![name, uid])?;
            }
        }
        for table in ["findings", "review_events"] {
            tx.execute(
                &format!(
                    "UPDATE {table} SET decision_uid = (SELECT uid FROM decision_uids WHERE name = {table}.rule_id) \
                     WHERE decision_uid IS NULL \
                       AND EXISTS (SELECT 1 FROM decision_uids WHERE name = {table}.rule_id)"
                ),
                [],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Imports the events of the decision log the cache does not have, and
    /// exports the events only the cache has (from before the log existed) to
    /// the log. Importing is idempotent: events are identified by content.
    fn sync(&mut self) -> Result<(), Error> {
        let Some(path) = self.log.clone() else {
            return Ok(());
        };
        let read = log::read(&path)?;
        if read.skipped > 0 {
            self.notices.push(format!(
                "{} record(s) in {} are of a kind or version this build does not know and were skipped; a newer lighthouse wrote them",
                read.skipped,
                path.display()
            ));
        }
        let logged = read.events;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for rewrite in &read.rewrites {
            apply_rewrite(&tx, rewrite)?;
        }
        for event in &logged {
            insert_event(&tx, event)?;
        }
        let known: BTreeSet<&str> = logged.iter().map(|e| e.id.as_str()).collect();
        let only_here: Vec<ReviewEvent> = all_events(&tx)?
            .into_iter()
            .filter(|e| !known.contains(e.id.as_str()))
            .collect();
        tx.commit()?;
        only_here
            .iter()
            .try_for_each(|event| log::append(&path, event))
    }
}

/// The quality store of one project. Findings and their sightings live in a
/// SQLite file that is a local cache; verdicts live in the committed decision
/// log, which the cache is synchronized from whenever the store is opened, so
/// that anyone with the log reaches the same suppressions. Safe to share
/// between processes.
pub struct Store {
    conn: Connection,
    log: Option<PathBuf>,
    notices: Vec<String>,
}

fn retry_busy<T>(mut attempt: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
    let mut tries = 0;
    loop {
        match attempt() {
            Err(Error::Sqlite(e))
                if tries < OPEN_ATTEMPTS
                    && matches!(
                        e.sqlite_error_code(),
                        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                    ) =>
            {
                tries += 1;
                thread::sleep(OPEN_PAUSE);
            }
            other => return other,
        }
    }
}

/// Names the file when SQLite says it is not a database.
fn diagnose(error: Error, path: &Path) -> Error {
    match &error {
        Error::Sqlite(e)
            if matches!(
                e.sqlite_error_code(),
                Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
            ) =>
        {
            Error::Corrupt {
                path: path.display().to_string(),
            }
        }
        _ => error,
    }
}

fn timestamp(conn: &Connection) -> Result<String, Error> {
    Ok(conn.query_row(&format!("SELECT {NOW}"), [], |row| row.get(0))?)
}

fn upsert(
    tx: &Transaction,
    observed: &Observed,
    run: &Run,
    now: &str,
    summary: &mut RunSummary,
) -> Result<(), Error> {
    let was_resolved: Option<bool> = tx
        .prepare_cached("SELECT resolved_at IS NOT NULL FROM findings WHERE fingerprint = ?1")?
        .query_row(params![observed.fingerprint], |row| row.get(0))
        .optional()?;
    match was_resolved {
        None => summary.opened += 1,
        Some(true) => summary.reopened += 1,
        Some(false) => {}
    }
    tx.prepare_cached(
        "INSERT INTO findings (fingerprint, rule_id, decision_uid, last_severity, authored_severity, path, locator, symbol, \
             first_seen, last_seen, last_message, last_evidence, last_facts, last_options, \
             last_commit, last_dirty, lighthouse_version, catalog_version, rule_version, \
             legacy_rule_version, check_revision, decision_hash, evidence_digest) \
         VALUES (:fingerprint, :rule_id, :decision_uid, :severity, :authored, :path, :locator, :symbol, :now, :now, \
             :message, :evidence, :facts, :options, :commit, :dirty, :version, :catalog, \
             :rule_version, :legacy_rule_version, :check_revision, :decision_hash, :digest) \
         ON CONFLICT (fingerprint) DO UPDATE SET \
             rule_id = excluded.rule_id, \
             decision_uid = COALESCE(excluded.decision_uid, decision_uid), \
             last_severity = excluded.last_severity, \
             authored_severity = excluded.authored_severity, path = excluded.path, locator = excluded.locator, \
             symbol = excluded.symbol, last_seen = excluded.last_seen, \
             reopened = reopened + (resolved_at IS NOT NULL), resolved_at = NULL, \
             inactive_at = NULL, last_message = excluded.last_message, \
             last_evidence = excluded.last_evidence, last_facts = excluded.last_facts, \
             last_options = excluded.last_options, last_commit = excluded.last_commit, \
             last_dirty = excluded.last_dirty, lighthouse_version = excluded.lighthouse_version, \
             catalog_version = excluded.catalog_version, rule_version = excluded.rule_version, \
             legacy_rule_version = excluded.legacy_rule_version, \
             check_revision = excluded.check_revision, \
             decision_hash = excluded.decision_hash, evidence_digest = excluded.evidence_digest",
    )?
    .execute(
        named_params! {
            ":fingerprint": observed.fingerprint,
            ":rule_id": observed.rule_id,
            ":decision_uid": observed.decision_uid,
            ":severity": observed.severity.to_string(),
            ":authored": observed.authored_severity,
            ":path": observed.path,
            ":locator": observed.locator.to_string(),
            ":symbol": observed.symbol,
            ":now": now,
            ":message": observed.message,
            ":evidence": observed.evidence.to_string(),
            ":facts": observed.facts.to_string(),
            ":options": observed.options.to_string(),
            ":commit": run.commit,
            ":dirty": run.dirty,
            ":version": run.lighthouse_version,
            ":catalog": run.catalog_version,
            ":rule_version": observed.rule_version,
            ":legacy_rule_version": observed.legacy_rule_version,
            ":check_revision": observed.check_revision,
            ":decision_hash": observed.decision_hash,
            ":digest": observed.evidence_digest(),
        },
    )?;
    Ok(())
}

/// Moves what is kept under the fingerprint `legacy` to the one `observed`
/// has now: the finding's row in place (its history stays), its fix records,
/// and the verdicts, which are read as belonging to the new fingerprint from
/// here on. Returns the rewrite record to append to the decision log when
/// verdicts moved. Does nothing when nothing is kept under `legacy`, or when
/// the finding already has a row of its own.
fn adopt(
    tx: &Transaction,
    legacy: &str,
    observed: &Observed,
    summary: &mut RunSummary,
) -> Result<Option<RewriteSpec>, Error> {
    let current = observed.fingerprint.as_str();
    if legacy == current {
        return Ok(None);
    }
    let moved = tx.execute(
        "UPDATE findings SET fingerprint = ?2 WHERE fingerprint = ?1 \
         AND NOT EXISTS (SELECT 1 FROM findings WHERE fingerprint = ?2)",
        params![legacy, current],
    )?;
    if moved > 0 {
        tx.execute(
            "UPDATE fix_events SET fingerprint = ?2 WHERE fingerprint = ?1",
            params![legacy, current],
        )?;
        summary.rewritten += 1;
    }
    let verdicts: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM review_events WHERE fingerprint = ?1) \
         AND NOT EXISTS (SELECT 1 FROM fingerprint_rewrites WHERE legacy = ?1)",
        params![legacy],
        |row| row.get(0),
    )?;
    if !verdicts {
        return Ok(None);
    }
    let rewrite = RewriteSpec {
        legacy: legacy.to_owned(),
        current: current.to_owned(),
        decision_uid: observed.decision_uid.clone(),
    };
    apply_rewrite(tx, &rewrite)?;
    summary.rewritten += 1;
    Ok(Some(rewrite))
}

/// Reads the verdicts recorded under `rewrite.legacy` as belonging to
/// `rewrite.current`. The first rewrite of a fingerprint wins.
fn apply_rewrite(tx: &Transaction, rewrite: &RewriteSpec) -> Result<(), Error> {
    tx.execute(
        "INSERT OR IGNORE INTO fingerprint_rewrites (legacy, current, decision_uid) \
         VALUES (?1, ?2, ?3)",
        params![rewrite.legacy, rewrite.current, rewrite.decision_uid],
    )?;
    tx.execute(
        "UPDATE review_events SET subject = \
             (SELECT current FROM fingerprint_rewrites WHERE legacy = ?1) \
         WHERE fingerprint = ?1",
        params![rewrite.legacy],
    )?;
    Ok(())
}

fn resolve_absent(tx: &Transaction, run: &Run, now: &str) -> Result<usize, Error> {
    if run.unchecked == Unchecked::Everything {
        return Ok(0);
    }
    let seen: BTreeSet<&str> = run
        .observed
        .iter()
        .map(|o| o.fingerprint.as_str())
        .collect();
    let mut stmt =
        tx.prepare("SELECT fingerprint, rule_id, path FROM findings WHERE resolved_at IS NULL")?;
    let open = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut gone = Vec::new();
    for entry in open {
        let (fingerprint, rule, path) = entry?;
        let in_scope = run.rules.contains(&rule)
            && run.reported.iter().any(|p| Path::new(&path).starts_with(p))
            && !unchecked(run, &path);
        if in_scope && !seen.contains(fingerprint.as_str()) {
            gone.push(fingerprint);
        }
    }
    for fingerprint in &gone {
        tx.execute(
            "UPDATE findings SET resolved_at = ?2 WHERE fingerprint = ?1",
            params![fingerprint, now],
        )?;
    }
    Ok(gone.len())
}

/// Whether the run could not check the file at `path`.
fn unchecked(run: &Run, path: &str) -> bool {
    match &run.unchecked {
        Unchecked::Nothing => false,
        Unchecked::Everything => true,
        Unchecked::Paths(dirs) => dirs.iter().any(|dir| Path::new(path).starts_with(dir)),
    }
}

/// Marks open findings of rules the configuration no longer enables as
/// inactive, and clears the mark of those whose rule is enabled again.
fn reconcile_inactive(tx: &Transaction, run: &Run, now: &str) -> Result<usize, Error> {
    let mut stmt = tx.prepare(
        "SELECT fingerprint, rule_id, inactive_at IS NOT NULL FROM findings \
         WHERE resolved_at IS NULL OR inactive_at IS NOT NULL",
    )?;
    let candidates = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, bool>(2)?,
        ))
    })?;
    let mut deactivated = 0;
    for entry in candidates {
        let (fingerprint, rule, inactive) = entry?;
        let configured = run.configured.contains(&rule);
        if configured == inactive {
            let value = (!configured).then_some(now);
            tx.execute(
                "UPDATE findings SET inactive_at = ?2 WHERE fingerprint = ?1",
                params![fingerprint, value],
            )?;
            deactivated += usize::from(!configured);
        }
    }
    Ok(deactivated)
}

/// The full fingerprint that `prefix` names, among findings and events.
fn expand(conn: &Connection, prefix: &str) -> Result<String, Error> {
    let mut stmt = conn.prepare(
        "SELECT fingerprint FROM findings WHERE substr(fingerprint, 1, length(?1)) = ?1 \
         UNION SELECT subject FROM review_events WHERE substr(subject, 1, length(?1)) = ?1 \
         ORDER BY 1 LIMIT 6",
    )?;
    let mut found: Vec<String> = stmt
        .query_map(params![prefix], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    match found.len() {
        0 => Err(Error::UnknownFinding(prefix.to_owned())),
        1 => Ok(found.remove(0)),
        _ => {
            found.truncate(MAX_CANDIDATES);
            Err(Error::AmbiguousFinding {
                prefix: prefix.to_owned(),
                candidates: found,
            })
        }
    }
}

fn load_finding(conn: &Connection, prefix: &str) -> Result<FindingRecord, Error> {
    let full = expand(conn, prefix)?;
    conn.query_row(
        "SELECT * FROM finding_states WHERE fingerprint = ?1",
        params![full],
        finding_record,
    )
    .optional()?
    .ok_or_else(|| Error::UnknownFinding(prefix.to_owned()))
}

fn all_events(conn: &Connection) -> Result<Vec<ReviewEvent>, Error> {
    let sql = format!("SELECT {EVENT_COLUMNS} FROM review_events ORDER BY timestamp, event_id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], review_event)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The event that recording `review` on `finding` writes: the finding frozen
/// as it was last seen, sealed with the id its content determines.
fn event_of(
    review: &NewReview,
    finding: &FindingRecord,
    stamp: Stamp,
    timestamp: String,
) -> Result<ReviewEvent, Error> {
    let snapshot = json!({
        "v": SNAPSHOT_VERSION,
        "message": finding.message,
        "path": finding.path,
        "locator": finding.locator,
        "symbol": finding.symbol,
        "evidence": finding.evidence,
        "facts": finding.facts,
        "options": finding.options,
        "severity": finding.severity,
        "authored": finding.authored_severity,
        "seenAt": finding.last_seen,
        "commit": finding.commit,
        "dirty": finding.dirty,
        "lighthouseVersion": finding.lighthouse_version,
        "patternFingerprint": Value::Null,
    });
    let mut event = ReviewEvent {
        id: String::new(),
        fingerprint: finding.fingerprint.clone(),
        rule_id: finding.rule_id.clone(),
        decision_uid: stamp.decision_uid.or_else(|| finding.decision_uid.clone()),
        rule_version: stamp.rule_version,
        check_revision: stamp.check_revision,
        decision_hash: stamp.decision_hash,
        catalog_version: stamp.catalog_version,
        lighthouse_version: Some(review.lighthouse_version.clone()),
        pattern_fingerprint: None,
        verdict: review.verdict,
        reason: review.reason,
        reason_text: review.reason_text.clone(),
        reviewer_kind: review.reviewer_kind,
        reviewer_id: review.reviewer_id.clone(),
        language: finding
            .facts
            .get("language")
            .and_then(Value::as_str)
            .map(str::to_owned),
        scope: stamp.scope,
        evidence_digest: Some(digest::evidence(&finding.evidence)),
        snapshot,
        commit: review.commit.clone(),
        timestamp,
    };
    log::seal(&mut event)?;
    Ok(event)
}

fn insert_event(tx: &Transaction, event: &ReviewEvent) -> Result<(), Error> {
    tx.execute(
        "INSERT OR IGNORE INTO review_events (event_id, fingerprint, rule_id, decision_uid, rule_version, check_revision, \
             decision_hash, catalog_version, lighthouse_version, pattern_fingerprint, verdict, \
             reason_code, reason_text, reviewer_kind, reviewer_id, language, scope, \
             evidence_digest, feature_snapshot, git_commit, timestamp, subject) \
         VALUES (:id, :fingerprint, :rule_id, :decision_uid, :rule_version, :check_revision, :decision_hash, :catalog, :version, \
             :pattern_fingerprint, :verdict, :reason, :reason_text, :kind, :reviewer, :language, \
             :scope, :digest, :snapshot, :commit, :timestamp, \
             COALESCE((SELECT current FROM fingerprint_rewrites WHERE legacy = :fingerprint), :fingerprint))",
        named_params! {
            ":id": event.id,
            ":fingerprint": event.fingerprint,
            ":rule_id": event.rule_id,
            ":decision_uid": event.decision_uid,
            ":rule_version": event.rule_version,
            ":check_revision": event.check_revision,
            ":decision_hash": event.decision_hash,
            ":catalog": event.catalog_version,
            ":version": event.lighthouse_version,
            ":pattern_fingerprint": event.pattern_fingerprint,
            ":verdict": event.verdict.as_str(),
            ":reason": event.reason.as_str(),
            ":reason_text": event.reason_text,
            ":kind": event.reviewer_kind.as_str(),
            ":reviewer": event.reviewer_id,
            ":language": event.language,
            ":scope": event.scope,
            ":digest": event.evidence_digest,
            ":snapshot": event.snapshot.to_string(),
            ":commit": event.commit,
            ":timestamp": event.timestamp,
        },
    )?;
    Ok(())
}

fn finding_record(row: &Row) -> rusqlite::Result<FindingRecord> {
    let verdict: Option<String> = row.get("review_verdict")?;
    let reason: Option<String> = row.get("review_reason")?;
    let review = match (verdict, reason) {
        (Some(verdict), Some(reason)) => Some(LatestReview {
            verdict: parse(&verdict)?,
            reason: parse(&reason)?,
        }),
        _ => None,
    };
    let standing: Option<String> = row.get("standing")?;
    Ok(FindingRecord {
        fingerprint: row.get("fingerprint")?,
        rule_id: row.get("rule_id")?,
        decision_uid: row.get("decision_uid")?,
        severity: row.get("last_severity")?,
        authored_severity: row.get("authored_severity")?,
        path: row.get("path")?,
        locator: json_column(row, "locator")?,
        symbol: row.get("symbol")?,
        first_seen: row.get("first_seen")?,
        last_seen: row.get("last_seen")?,
        resolved_at: row.get("resolved_at")?,
        inactive_at: row.get("inactive_at")?,
        reopened: row.get("reopened")?,
        message: row.get("last_message")?,
        evidence: json_column(row, "last_evidence")?,
        facts: json_column(row, "last_facts")?,
        options: json_column(row, "last_options")?,
        commit: row.get("last_commit")?,
        dirty: row.get("last_dirty")?,
        lighthouse_version: row.get("lighthouse_version")?,
        catalog_version: row.get("catalog_version")?,
        rule_version: row.get("rule_version")?,
        review,
        standing: standing.as_deref().and_then(Standing::parse),
        narrowing: row.get::<_, i64>("narrowing")? != 0,
    })
}

fn review_event(row: &Row) -> rusqlite::Result<ReviewEvent> {
    Ok(ReviewEvent {
        id: row.get("event_id")?,
        fingerprint: row.get("fingerprint")?,
        rule_id: row.get("rule_id")?,
        decision_uid: row.get("decision_uid")?,
        rule_version: row.get("rule_version")?,
        check_revision: row.get("check_revision")?,
        decision_hash: row.get("decision_hash")?,
        catalog_version: row.get("catalog_version")?,
        lighthouse_version: row.get("lighthouse_version")?,
        pattern_fingerprint: row.get("pattern_fingerprint")?,
        verdict: parse(&row.get::<_, String>("verdict")?)?,
        reason: parse(&row.get::<_, String>("reason_code")?)?,
        reason_text: row.get("reason_text")?,
        reviewer_kind: parse(&row.get::<_, String>("reviewer_kind")?)?,
        reviewer_id: row.get("reviewer_id")?,
        language: row.get("language")?,
        scope: row.get("scope")?,
        evidence_digest: row.get("evidence_digest")?,
        snapshot: json_column(row, "feature_snapshot")?,
        commit: row.get("git_commit")?,
        timestamp: row.get("timestamp")?,
    })
}

fn json_column(row: &Row, name: &str) -> rusqlite::Result<Value> {
    let text: String = row.get(name)?;
    serde_json::from_str(&text).map_err(conversion)
}

fn parse<T: std::str::FromStr>(text: &str) -> rusqlite::Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    text.parse().map_err(conversion)
}

fn conversion(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(error))
}

fn unknown(what: &(impl std::fmt::Display + ?Sized)) -> Error {
    Error::Sqlite(rusqlite::Error::InvalidColumnName(format!(
        "unexpected value `{what}`"
    )))
}

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}
