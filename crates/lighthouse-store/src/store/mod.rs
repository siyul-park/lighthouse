use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use lighthouse_model::Judgment;
use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior, named_params,
    params,
};

use lighthouse_resource::STATE_DIR;

use crate::{
    Error, Filter, FindingRecord, JudgmentEvent, NewJudgment, Observed, Resolved, Ruling, Run,
    RunSummary, Subject, SuppressionEvent, Unchecked, Versions, log, schema,
};

mod rows;

use rows::*;

/// File name of the database inside the store directory.
const FILE: &str = "lighthouse.db";
/// How long a writer waits for another process holding the database.
const BUSY: Duration = Duration::from_secs(5);
/// Attempts to switch a database that another process is switching, and the
/// pause between them.
const OPEN_ATTEMPTS: u32 = 100;
const OPEN_PAUSE: Duration = Duration::from_millis(20);
/// Format version of the feature snapshot of a judgment.
const SNAPSHOT_VERSION: u32 = 2;

const JUDGMENT_COLUMNS: &str = "judgment_id, fingerprint, decision, decision_uid, meaning_version, \
     check_revision, model_version, decision_hash, catalog_version, lighthouse_version, judgment, reason, \
     agent_type, agent_id, agent_role, language, scope, evidence_digest, feature_snapshot, git_commit, \
     generated_at";

/// Most fingerprints an ambiguous prefix error lists.
const MAX_CANDIDATES: usize = 5;

impl Store {
    /// Where a project's cache lives: `<root>/.lighthouse/lighthouse.db`.
    pub fn path_in(root: &Path) -> PathBuf {
        root.join(STATE_DIR).join(FILE)
    }

    /// Where a project's decision log lives: `<root>/.lighthouse/decisions.jsonl`.
    pub fn log_path_in(root: &Path) -> PathBuf {
        root.join(STATE_DIR).join(log::FILE)
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
    /// reported paths and decisions and are not seen now are resolved, except
    /// where the run could not check; findings of decisions the configuration
    /// no longer enables become inactive.
    pub fn record(&mut self, run: &Run) -> Result<RunSummary, Error> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = lighthouse_model::now();
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
        let standings = standings(&self.conn)?;
        Ok(subjects
            .iter()
            .filter_map(|s| Some((s.fingerprint.clone(), standings.ruling(s)?)))
            .collect())
    }

    /// The findings matching `filter`, by path and first sighting.
    pub fn list(&self, filter: &Filter) -> Result<Vec<FindingRecord>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM findings WHERE (?1 IS NULL OR decision = ?1) \
             ORDER BY path, first_seen, fingerprint",
        )?;
        let standings = standings(&self.conn)?;
        let rows = stmt.query_map(params![filter.decision], |row| {
            finding_record(row, &standings)
        })?;
        let found: Vec<FindingRecord> = rows.collect::<Result<_, _>>()?;
        Ok(found
            .into_iter()
            .filter(|f| filter.status.includes(f))
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
    /// of its last sighting are frozen as the feature snapshot. `versions`
    /// supplies the versions of the finding's decision. A suppression belongs
    /// to a `fail`, and when `expect_seen` is set the finding must not have
    /// been seen again since.
    pub fn resolve(
        &mut self,
        review: &NewJudgment,
        versions: impl FnOnce(&FindingRecord) -> Versions,
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
        let mut event = event_of(
            review,
            &finding,
            versions(&finding),
            lighthouse_model::now(),
        )?;
        if let Some(justification) = &review.suppress {
            let suppression = SuppressionEvent::external(&event, justification)?;
            event.suppressions.push(suppression);
        }
        if let Some(path) = &self.log {
            let lines = std::iter::once(log::line(&event.id, event.spec.clone()))
                .chain(
                    event
                        .suppressions
                        .iter()
                        .map(|s| log::line(&s.id, s.spec.clone())),
                )
                .collect::<Result<Vec<_>, _>>()?;
            log::append(path, &lines)?;
        }
        insert_judgment(&tx, &event)?;
        tx.commit()?;
        Ok(Resolved { event, finding })
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
            .flat_map(|e| {
                std::iter::once(e.id.as_str()).chain(e.suppressions.iter().map(|s| s.id.as_str()))
            })
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
        "INSERT INTO findings (fingerprint, decision, decision_uid, severity, authored_severity, path, locator, symbol, \
             first_seen, last_seen, last_message, last_evidence, last_facts, last_options, \
             last_commit, last_dirty, lighthouse_version, catalog_version, meaning_version, \
             check_revision, model_version, decision_hash, evidence_digest) \
         VALUES (:fingerprint, :decision, :decision_uid, :severity, :authored, :path, :locator, :symbol, :now, :now, \
             :message, :evidence, :facts, :options, :commit, :dirty, :version, :catalog, \
             :meaning_version, :check_revision, :model_version, :decision_hash, :digest) \
         ON CONFLICT (fingerprint) DO UPDATE SET \
             decision = excluded.decision, \
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
             check_revision = excluded.check_revision, model_version = excluded.model_version, \
             decision_hash = excluded.decision_hash, evidence_digest = excluded.evidence_digest",
    )?
    .execute(
        named_params! {
            ":fingerprint": observed.fingerprint,
            ":decision": observed.decision,
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
            ":model_version": observed.model_version,
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
        tx.prepare("SELECT fingerprint, decision, path FROM findings WHERE resolved_at IS NULL")?;
    let open = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut gone = Vec::new();
    for entry in open {
        let (fingerprint, decision, path) = entry?;
        let in_scope = run.ran.contains(&decision)
            && run.reported.iter().any(|p| Path::new(&path).starts_with(p))
            && !run.unchecked.covers(&path);
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

/// Marks open findings of decisions the configuration no longer enables as
/// inactive, and clears the mark of those whose decision is enabled again.
fn reconcile_inactive(tx: &Transaction, run: &Run, now: &str) -> Result<usize, Error> {
    let mut stmt = tx.prepare(
        "SELECT fingerprint, decision, inactive_at IS NOT NULL FROM findings \
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
        let (fingerprint, decision, inactive) = entry?;
        let active = run.active.contains(&decision);
        if active == inactive {
            let value = (!active).then_some(now);
            tx.execute(
                "UPDATE findings SET inactive_at = ?2 WHERE fingerprint = ?1",
                params![fingerprint, value],
            )?;
            deactivated += usize::from(!active);
        }
    }
    Ok(deactivated)
}
