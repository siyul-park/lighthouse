use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};

use lighthouse_model::{Attribution, Judgment, Severity, hash};
use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Row, Transaction, TransactionBehavior, named_params,
    params, types::Type,
};
use serde_json::{Value, json};

use crate::{
    Error, Filter, FindingRecord, FixEvent, JudgmentEvent, NewFix, NewJudgment, Observed, Resolved,
    Ruling, Run, RunSummary, Stamp, Standing, StatusFilter, Subject, SuppressionEvent, Unchecked,
    digest,
    judged::Judged,
    log,
    log::{JudgmentSpec, LoggedSuppression},
    schema,
};
use lighthouse_model::{SuppressionKind, SuppressionStatus};

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
/// Format version of the feature snapshot of a judgment.
const SNAPSHOT_VERSION: u32 = 2;

const JUDGMENT_COLUMNS: &str = "judgment_id, fingerprint, rule_id, decision_uid, meaning_version, \
     check_revision, decision_hash, catalog_version, lighthouse_version, judgment, reason, \
     agent_type, agent_id, language, scope, evidence_digest, feature_snapshot, git_commit, \
     generated_at";

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
    /// directory, building its schema (a cache of another schema version is
    /// emptied) and importing the decision log.
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
        schema::prepare(&mut conn)?;
        let mut store = Self {
            conn,
            log,
            notices: Vec::new(),
        };
        store.sync()?;
        Ok(store)
    }

    /// What opening the store skipped in the log: a last line an interrupted
    /// write cut short, a suppression that goes with no judgment.
    pub fn notices(&self) -> &[String] {
        &self.notices
    }

    /// The schema version of the database.
    pub fn schema_version(&self) -> Result<i64, Error> {
        schema::version(&self.conn)
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
            match upsert(&tx, observed, run, &now)? {
                Seen::New => summary.opened += 1,
                Seen::Reopened => summary.reopened += 1,
                Seen::Open => {}
            }
        }
        summary.resolved = resolve_absent(&tx, run, &now)?;
        summary.deactivated = reconcile_inactive(&tx, run, &now)?;
        tx.commit()?;
        Ok(summary)
    }

    /// What the judgment that stands does to each of `subjects`, the findings a
    /// run saw: it keeps one out of reports, stands without hiding it, has
    /// expired because the decision or the evidence moved, or cannot hide it
    /// because the finding is an error (its authored severity, not its
    /// severity, decides). It is computed from the judgments alone, so it holds
    /// on a clone that has no findings recorded yet.
    pub fn standings_for(&self, subjects: &[Subject]) -> Result<BTreeMap<String, Ruling>, Error> {
        let judged = Judged::load(&self.conn)?;
        Ok(subjects
            .iter()
            .filter_map(|s| Some((s.fingerprint.clone(), judged.ruling(s)?)))
            .collect())
    }

    /// The findings matching `filter`, by path and first sighting.
    pub fn list(&self, filter: &Filter) -> Result<Vec<FindingRecord>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM findings WHERE (?1 IS NULL OR rule_id = ?1) \
             ORDER BY path, first_seen, fingerprint",
        )?;
        let judged = Judged::load(&self.conn)?;
        let rows = stmt.query_map(params![filter.rule], |row| finding_record(row, &judged))?;
        let found: Vec<FindingRecord> = rows.collect::<Result<_, _>>()?;
        Ok(found
            .into_iter()
            .filter(|f| listed(f, filter.status))
            .collect())
    }

    /// The finding whose fingerprint is, or uniquely starts with, `fingerprint`.
    pub fn finding(&self, fingerprint: &str) -> Result<FindingRecord, Error> {
        load_finding(&self.conn, fingerprint)
    }

    /// The judgments on a finding, oldest first. The finding need not have
    /// been seen on this machine: a judgment from the decision log is enough.
    pub fn history(&self, fingerprint: &str) -> Result<Vec<JudgmentEvent>, Error> {
        let full = expand(&self.conn, fingerprint)?;
        judgments_of(&self.conn, Some(&full))
    }

    /// Appends a judgment, and the suppression that goes with it, to the
    /// decision log, then to the cache. The finding is read inside the
    /// transaction that records the judgment, and its evidence and the facts
    /// of its last sighting are frozen as the feature snapshot. `stamp`
    /// supplies the versions of the finding's decision. A suppression belongs
    /// to a `fail`, and when `expect_seen` is set the finding must not have
    /// been seen again since.
    pub fn resolve(
        &mut self,
        review: &NewJudgment,
        stamp: impl FnOnce(&FindingRecord) -> Stamp,
    ) -> Result<Resolved, Error> {
        if review.suppress.is_some() && review.judgment != Judgment::Fail {
            return Err(Error::SuppressionWithoutFail(review.judgment));
        }
        if review
            .suppress
            .as_deref()
            .is_some_and(|j| j.trim().is_empty())
        {
            return Err(Error::EmptyJustification);
        }
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
        let suppression = review
            .suppress
            .as_ref()
            .map(|justification| suppression_of(&event, justification))
            .transpose()?;
        if let Some(path) = &self.log {
            let mut lines = vec![log::line(&event.id, JudgmentSpec::from(&event))?];
            if let Some(s) = &suppression {
                lines.push(log::line(&s.event.id, s.spec())?);
            }
            log::append(path, &lines)?;
        }
        insert_judgment(&tx, &event)?;
        if let Some(s) = &suppression {
            insert_suppression(&tx, s)?;
        }
        tx.commit()?;
        let mut event = event;
        event.suppressions = suppression.into_iter().map(|s| s.event).collect();
        Ok(Resolved { event, finding })
    }

    /// Records that a fixer changed the code for a finding. The record is
    /// local: unlike a judgment it is not written to the decision log.
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
                env!("CARGO_PKG_VERSION"),
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
                files: serde_json::from_str(&files).map_err(conversion)?,
                commit: row.get(7)?,
                timestamp: row.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Deletes findings that are resolved or inactive, were never judged and
    /// have been so for at least `older_than_days` days (any time when `None`).
    /// Judged findings stay: their judgments are labels. Returns how many
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
             AND NOT EXISTS (SELECT 1 FROM judgments j WHERE j.fingerprint = findings.fingerprint) \
             {age}"
        );
        Ok(self.conn.execute(&sql, [])?)
    }

    /// Makes the cache hold exactly what the decision log holds, the only source
    /// of truth: records the log has are imported, records it lacks (a branch
    /// switch, a removed line) are dropped from the cache. The log is read after
    /// the write lock is taken, so a record being appended is never half seen.
    /// Nothing is ever written to the log from here.
    fn sync(&mut self) -> Result<(), Error> {
        let Some(path) = self.log.clone() else {
            return Ok(());
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let read = log::read(&path)?;
        self.notices.extend(read.notices);
        let wanted: BTreeSet<&str> = read
            .judgments
            .iter()
            .map(|e| e.id.as_str())
            .chain(read.suppressions.iter().map(|s| s.event.id.as_str()))
            .collect();
        let mut stale: Vec<(&str, &str, String)> = Vec::new();
        for (table, column) in [
            ("judgments", "judgment_id"),
            ("suppressions", "suppression_id"),
        ] {
            let mut stmt = tx.prepare(&format!("SELECT {column} FROM {table}"))?;
            let ids = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for id in ids {
                let id = id?;
                if !wanted.contains(id.as_str()) {
                    stale.push((table, column, id));
                }
            }
        }
        if !stale.is_empty() {
            schema::lift_guards(&tx)?;
            for (table, column, id) in &stale {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE {column} = ?1"),
                    params![id],
                )?;
            }
            schema::restore_guards(&tx)?;
        }
        for event in &read.judgments {
            insert_judgment(&tx, event)?;
        }
        for suppression in &read.suppressions {
            insert_suppression(&tx, suppression)?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// The quality store of one project. Findings and their sightings live in a
/// SQLite file that is a local cache; judgments and suppressions live in the
/// committed decision log, which the cache is synchronized from whenever the
/// store is opened, so that anyone with the log reaches the same standings.
/// Safe to share between processes.
pub struct Store {
    conn: Connection,
    log: Option<PathBuf>,
    notices: Vec<String>,
}

/// What the store knew of a finding before a run saw it again.
enum Seen {
    New,
    Reopened,
    Open,
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

fn upsert(tx: &Transaction, observed: &Observed, run: &Run, now: &str) -> Result<Seen, Error> {
    let was_resolved: Option<bool> = tx
        .prepare_cached("SELECT resolved_at IS NOT NULL FROM findings WHERE fingerprint = ?1")?
        .query_row(params![observed.fingerprint], |row| row.get(0))
        .optional()?;
    let seen = match was_resolved {
        None => Seen::New,
        Some(true) => Seen::Reopened,
        Some(false) => Seen::Open,
    };
    tx.prepare_cached(
        "INSERT INTO findings (fingerprint, rule_id, decision_uid, severity, authored_severity, path, locator, symbol, \
             first_seen, last_seen, last_message, last_evidence, last_facts, last_options, \
             last_commit, last_dirty, lighthouse_version, catalog_version, meaning_version, \
             check_revision, decision_hash, evidence_digest) \
         VALUES (:fingerprint, :rule_id, :decision_uid, :severity, :authored, :path, :locator, :symbol, :now, :now, \
             :message, :evidence, :facts, :options, :commit, :dirty, :version, :catalog, \
             :meaning_version, :check_revision, :decision_hash, :digest) \
         ON CONFLICT (fingerprint) DO UPDATE SET \
             rule_id = excluded.rule_id, \
             decision_uid = COALESCE(excluded.decision_uid, decision_uid), \
             severity = excluded.severity, \
             authored_severity = excluded.authored_severity, path = excluded.path, locator = excluded.locator, \
             symbol = excluded.symbol, last_seen = excluded.last_seen, \
             reopened = reopened + (resolved_at IS NOT NULL), resolved_at = NULL, \
             inactive_at = NULL, last_message = excluded.last_message, \
             last_evidence = excluded.last_evidence, last_facts = excluded.last_facts, \
             last_options = excluded.last_options, last_commit = excluded.last_commit, \
             last_dirty = excluded.last_dirty, lighthouse_version = excluded.lighthouse_version, \
             catalog_version = excluded.catalog_version, meaning_version = excluded.meaning_version, \
             check_revision = excluded.check_revision, \
             decision_hash = excluded.decision_hash, evidence_digest = excluded.evidence_digest",
    )?
    .execute(
        named_params! {
            ":fingerprint": observed.fingerprint,
            ":rule_id": observed.rule_id,
            ":decision_uid": observed.decision_uid,
            ":severity": observed.severity.to_string(),
            ":authored": observed.authored_severity.to_string(),
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
            ":meaning_version": observed.meaning_version,
            ":check_revision": observed.check_revision,
            ":decision_hash": observed.decision_hash,
            ":digest": observed.evidence_digest(),
        },
    )?;
    Ok(seen)
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

/// The full fingerprint that `prefix` names, among findings and judgments.
fn expand(conn: &Connection, prefix: &str) -> Result<String, Error> {
    let mut stmt = conn.prepare(
        "SELECT fingerprint FROM findings WHERE substr(fingerprint, 1, length(?1)) = ?1 \
         UNION SELECT fingerprint FROM judgments WHERE substr(fingerprint, 1, length(?1)) = ?1 \
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
    let judged = Judged::load(conn)?;
    conn.query_row(
        "SELECT * FROM findings WHERE fingerprint = ?1",
        params![full],
        |row| finding_record(row, &judged),
    )
    .optional()?
    .ok_or_else(|| Error::UnknownFinding(prefix.to_owned()))
}

/// Whether a finding belongs to a listing of `status`.
fn listed(finding: &FindingRecord, status: StatusFilter) -> bool {
    let hidden = finding.standing == Some(Standing::Suppressed);
    match status {
        StatusFilter::Open => {
            finding.resolved_at.is_none() && finding.inactive_at.is_none() && !hidden
        }
        StatusFilter::Suppressed => hidden,
        StatusFilter::Narrowing => hidden && finding.narrowing,
        StatusFilter::Inactive => finding.inactive_at.is_some() && !hidden,
        StatusFilter::Resolved => finding.resolved_at.is_some(),
        StatusFilter::All => true,
    }
}

/// The judgments of one finding (every finding when `None`), oldest first,
/// each with its suppressions.
fn judgments_of(conn: &Connection, fingerprint: Option<&str>) -> Result<Vec<JudgmentEvent>, Error> {
    let sql = format!(
        "SELECT {JUDGMENT_COLUMNS} FROM judgments WHERE (?1 IS NULL OR fingerprint = ?1) \
         ORDER BY generated_at, judgment_id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![fingerprint], judgment_event)?;
    let mut events: Vec<JudgmentEvent> = rows.collect::<Result<_, _>>()?;
    let mut by_judgment: BTreeMap<String, Vec<SuppressionEvent>> = BTreeMap::new();
    for s in suppressions_of(conn)? {
        by_judgment.entry(s.judgment).or_default().push(s.event);
    }
    for event in &mut events {
        event.suppressions = by_judgment.remove(&event.id).unwrap_or_default();
    }
    Ok(events)
}

fn suppressions_of(conn: &Connection) -> Result<Vec<LoggedSuppression>, Error> {
    let mut stmt = conn.prepare(
        "SELECT suppression_id, judgment_id, fingerprint, kind, status, justification, \
             agent_type, agent_id, generated_at FROM suppressions \
         ORDER BY generated_at, suppression_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(LoggedSuppression {
            judgment: row.get(1)?,
            fingerprint: row.get(2)?,
            event: SuppressionEvent {
                id: row.get(0)?,
                kind: parse(&row.get::<_, String>(3)?)?,
                status: parse(&row.get::<_, String>(4)?)?,
                justification: row.get(5)?,
                was_attributed_to: Attribution {
                    kind: parse(&row.get::<_, String>(6)?)?,
                    id: row.get(7)?,
                },
                generated_at_time: row.get(8)?,
            },
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The judgment that recording `review` on `finding` writes: the finding
/// frozen as it was last seen, sealed with the id its content determines.
fn event_of(
    review: &NewJudgment,
    finding: &FindingRecord,
    stamp: Stamp,
    timestamp: String,
) -> Result<JudgmentEvent, Error> {
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
    });
    let mut event = JudgmentEvent {
        id: String::new(),
        fingerprint: finding.fingerprint.clone(),
        decision_name: finding.rule_id.clone(),
        decision_uid: stamp.decision_uid.or_else(|| finding.decision_uid.clone()),
        meaning_version: stamp.meaning_version,
        check_revision: stamp.check_revision,
        decision_hash: stamp.decision_hash,
        catalog_version: stamp.catalog_version,
        lighthouse_version: Some(review.lighthouse_version.clone()),
        judgment: review.judgment,
        reason: review.reason.clone(),
        was_attributed_to: review.attribution.clone(),
        generated_at_time: timestamp,
        language: finding
            .facts
            .get("language")
            .and_then(Value::as_str)
            .map(str::to_owned),
        scope: stamp.scope,
        evidence_digest: Some(digest::evidence(&finding.evidence)),
        snapshot,
        commit: review.commit.clone(),
        suppressions: Vec::new(),
    };
    log::seal(&mut event)?;
    Ok(event)
}

/// The external suppression that goes with `event`, written by whoever wrote
/// the judgment, at the same time.
fn suppression_of(event: &JudgmentEvent, justification: &str) -> Result<LoggedSuppression, Error> {
    let mut suppression = LoggedSuppression {
        judgment: event.id.clone(),
        fingerprint: event.fingerprint.clone(),
        event: SuppressionEvent {
            id: String::new(),
            kind: SuppressionKind::External,
            status: SuppressionStatus::Accepted,
            justification: justification.to_owned(),
            was_attributed_to: event.was_attributed_to.clone(),
            generated_at_time: event.generated_at_time.clone(),
        },
    };
    suppression.event.id = log::id_of(&suppression.spec())?;
    Ok(suppression)
}

fn insert_judgment(tx: &Transaction, event: &JudgmentEvent) -> Result<(), Error> {
    tx.execute(
        "INSERT OR IGNORE INTO judgments (judgment_id, fingerprint, rule_id, decision_uid, \
             meaning_version, check_revision, decision_hash, catalog_version, lighthouse_version, \
             judgment, reason, agent_type, agent_id, language, scope, evidence_digest, \
             feature_snapshot, git_commit, generated_at) \
         VALUES (:id, :fingerprint, :rule_id, :decision_uid, :meaning_version, :check_revision, \
             :decision_hash, :catalog, :version, :judgment, :reason, :agent_type, :agent_id, \
             :language, :scope, :digest, :snapshot, :commit, :generated_at)",
        named_params! {
            ":id": event.id,
            ":fingerprint": event.fingerprint,
            ":rule_id": event.decision_name,
            ":decision_uid": event.decision_uid,
            ":meaning_version": event.meaning_version,
            ":check_revision": event.check_revision,
            ":decision_hash": event.decision_hash,
            ":catalog": event.catalog_version,
            ":version": event.lighthouse_version,
            ":judgment": event.judgment.as_str(),
            ":reason": event.reason,
            ":agent_type": event.was_attributed_to.kind.as_str(),
            ":agent_id": event.was_attributed_to.id,
            ":language": event.language,
            ":scope": event.scope,
            ":digest": event.evidence_digest,
            ":snapshot": event.snapshot.to_string(),
            ":commit": event.commit,
            ":generated_at": event.generated_at_time,
        },
    )?;
    Ok(())
}

fn insert_suppression(tx: &Transaction, s: &LoggedSuppression) -> Result<(), Error> {
    tx.execute(
        "INSERT OR IGNORE INTO suppressions (suppression_id, judgment_id, fingerprint, kind, \
             status, justification, agent_type, agent_id, generated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            s.event.id,
            s.judgment,
            s.fingerprint,
            s.event.kind.as_str(),
            s.event.status.as_str(),
            s.event.justification,
            s.event.was_attributed_to.kind.as_str(),
            s.event.was_attributed_to.id,
            s.event.generated_at_time,
        ],
    )?;
    Ok(())
}

fn finding_record(row: &Row, judged: &Judged) -> rusqlite::Result<FindingRecord> {
    let authored_severity = severity(row, "authored_severity")?;
    let subject = Subject {
        fingerprint: row.get("fingerprint")?,
        authored_severity,
        meaning_version: row.get("meaning_version")?,
        evidence_digest: row
            .get::<_, Option<String>>("evidence_digest")?
            .unwrap_or_default(),
    };
    let ruling = judged.ruling(&subject);
    Ok(FindingRecord {
        fingerprint: subject.fingerprint.clone(),
        rule_id: row.get("rule_id")?,
        decision_uid: row.get("decision_uid")?,
        severity: severity(row, "severity")?,
        authored_severity,
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
        meaning_version: subject.meaning_version,
        judgment: ruling.as_ref().map(|r| r.judgment),
        standing: ruling.as_ref().map(|r| r.standing),
        justification: ruling.as_ref().and_then(|r| r.justification.clone()),
        narrowing: ruling
            .as_ref()
            .is_some_and(|r| r.judgment == Judgment::NotApplicable),
    })
}

fn judgment_event(row: &Row) -> rusqlite::Result<JudgmentEvent> {
    Ok(JudgmentEvent {
        id: row.get("judgment_id")?,
        fingerprint: row.get("fingerprint")?,
        decision_name: row.get("rule_id")?,
        decision_uid: row.get("decision_uid")?,
        meaning_version: row.get("meaning_version")?,
        check_revision: row.get("check_revision")?,
        decision_hash: row.get("decision_hash")?,
        catalog_version: row.get("catalog_version")?,
        lighthouse_version: row.get("lighthouse_version")?,
        judgment: parse(&row.get::<_, String>("judgment")?)?,
        reason: row.get("reason")?,
        was_attributed_to: Attribution {
            kind: parse(&row.get::<_, String>("agent_type")?)?,
            id: row.get("agent_id")?,
        },
        generated_at_time: row.get("generated_at")?,
        language: row.get("language")?,
        scope: row.get("scope")?,
        evidence_digest: row.get("evidence_digest")?,
        snapshot: json_column(row, "feature_snapshot")?,
        commit: row.get("git_commit")?,
        suppressions: Vec::new(),
    })
}

fn severity(row: &Row, name: &str) -> rusqlite::Result<Severity> {
    parse(&row.get::<_, String>(name)?)
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

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}
