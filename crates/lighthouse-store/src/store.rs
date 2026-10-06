use std::{
    collections::BTreeSet,
    error::Error as StdError,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params, types::Type};
use serde_json::{Value, json};

use crate::{
    Error, Filter, FindingRecord, LatestReview, NewReview, Observed, ReviewEvent, Run, RunSummary,
    StatusFilter, migrations,
};

/// File name of the database inside the store directory.
const FILE: &str = "lighthouse.db";
/// Directory of a project that holds its Lighthouse state.
const DIR: &str = ".lighthouse";
/// How long a writer waits for another process holding the database.
const BUSY: Duration = Duration::from_secs(5);
/// SQL for the current UTC time, millisecond resolution.
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

const FINDING_COLUMNS: &str = "fingerprint, rule_id, severity, path, locator, symbol, first_seen, \
     last_seen, resolved_at, reopened, last_message, last_evidence, last_facts, verdict, \
     reason_code, suppressed";

const EVENT_COLUMNS: &str = "id, fingerprint, rule_id, rule_version, catalog_version, \
     pattern_fingerprint, verdict, reason_code, reason_text, reviewer_kind, reviewer_id, language, \
     scope, evidence, feature_snapshot, git_commit, timestamp";

/// The quality store of one project: findings with their history, and the
/// append-only log of review verdicts. One SQLite file, safe to share between
/// processes.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Where a project's store lives: `<root>/.lighthouse/lighthouse.db`.
    pub fn path_in(root: &Path) -> PathBuf {
        root.join(DIR).join(FILE)
    }

    /// Opens the store at `path`, creating the file and its directory and
    /// migrating the schema.
    pub fn open(path: &Path) -> Result<Self, Error> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|source| io(dir, source))?;
        }
        Self::connect(Connection::open(path)?)
    }

    /// Opens the store at `path` only if the file exists.
    pub fn open_existing(path: &Path) -> Result<Option<Self>, Error> {
        if !path.is_file() {
            return Ok(None);
        }
        Self::connect(Connection::open(path)?).map(Some)
    }

    /// A store that lives and dies with this value.
    pub fn open_in_memory() -> Result<Self, Error> {
        Self::connect(Connection::open_in_memory()?)
    }

    fn connect(mut conn: Connection) -> Result<Self, Error> {
        conn.busy_timeout(BUSY)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        migrations::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// The number of schema migrations the database has applied.
    pub fn schema_version(&self) -> Result<usize, Error> {
        migrations::version(&self.conn)
    }

    /// Records what a run saw. Seen findings are inserted or refreshed (a
    /// resolved one is reopened); findings that were open inside the run's
    /// reported paths and rules and are not seen now are resolved, but only
    /// when the run was complete.
    pub fn record(&mut self, run: &Run) -> Result<RunSummary, Error> {
        let tx = self.conn.transaction()?;
        let mut summary = RunSummary::default();
        for observed in &run.observed {
            upsert(&tx, observed, &mut summary)?;
        }
        if run.complete {
            summary.resolved = resolve_absent(&tx, run)?;
        }
        tx.commit()?;
        Ok(summary)
    }

    /// Fingerprints whose latest verdict is a rejection.
    pub fn suppressed(&self) -> Result<BTreeSet<String>, Error> {
        let mut stmt = self.conn.prepare("SELECT fingerprint FROM suppressions")?;
        let found = stmt.query_map([], |row| row.get(0))?;
        Ok(found.collect::<Result<_, _>>()?)
    }

    /// The findings matching `filter`, by path and first sighting.
    pub fn list(&self, filter: &Filter) -> Result<Vec<FindingRecord>, Error> {
        let status = match filter.status {
            StatusFilter::Open => "resolved_at IS NULL AND NOT suppressed",
            StatusFilter::Suppressed => "suppressed",
            StatusFilter::All => "1",
        };
        let sql = format!(
            "SELECT {FINDING_COLUMNS} FROM finding_states \
             WHERE (?1 IS NULL OR rule_id = ?1) AND {status} \
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

    /// The reviews of a finding, oldest first.
    pub fn history(&self, fingerprint: &str) -> Result<Vec<ReviewEvent>, Error> {
        let full = expand(&self.conn, fingerprint)?;
        let sql =
            format!("SELECT {EVENT_COLUMNS} FROM review_events WHERE fingerprint = ?1 ORDER BY id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![full], review_event)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Appends a verdict to the review log, freezing the finding's evidence
    /// and the facts of its last sighting as the feature snapshot. The verdict
    /// and reason must belong together.
    pub fn resolve(&mut self, review: &NewReview) -> Result<ReviewEvent, Error> {
        review.verdict.validate(review.reason)?;
        let tx = self.conn.transaction()?;
        let finding = load_finding(&tx, &review.fingerprint)?;
        let id = append(&tx, review, &finding)?;
        let event = load_event(&tx, id)?;
        tx.commit()?;
        Ok(event)
    }
}

fn upsert(tx: &Transaction, observed: &Observed, summary: &mut RunSummary) -> Result<(), Error> {
    let was_resolved: Option<bool> = tx
        .query_row(
            "SELECT resolved_at IS NOT NULL FROM findings WHERE fingerprint = ?1",
            params![observed.fingerprint],
            |row| row.get(0),
        )
        .optional()?;
    match was_resolved {
        None => summary.opened += 1,
        Some(true) => summary.reopened += 1,
        Some(false) => {}
    }
    tx.execute(
        &format!(
            "INSERT INTO findings (fingerprint, rule_id, severity, path, locator, symbol, \
                 first_seen, last_seen, last_message, last_evidence, last_facts) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, {NOW}, {NOW}, ?7, ?8, ?9) \
             ON CONFLICT (fingerprint) DO UPDATE SET \
                 rule_id = excluded.rule_id, severity = excluded.severity, \
                 path = excluded.path, locator = excluded.locator, symbol = excluded.symbol, \
                 last_seen = excluded.last_seen, \
                 reopened = reopened + (resolved_at IS NOT NULL), resolved_at = NULL, \
                 last_message = excluded.last_message, last_evidence = excluded.last_evidence, \
                 last_facts = excluded.last_facts"
        ),
        params![
            observed.fingerprint,
            observed.rule_id,
            observed.severity.to_string(),
            observed.path,
            observed.locator.to_string(),
            observed.symbol,
            observed.message,
            observed.evidence.to_string(),
            observed.facts.to_string(),
        ],
    )?;
    Ok(())
}

fn resolve_absent(tx: &Transaction, run: &Run) -> Result<usize, Error> {
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
            && run.reported.iter().any(|p| Path::new(&path).starts_with(p));
        if in_scope && !seen.contains(fingerprint.as_str()) {
            gone.push(fingerprint);
        }
    }
    for fingerprint in &gone {
        tx.execute(
            &format!("UPDATE findings SET resolved_at = {NOW} WHERE fingerprint = ?1"),
            params![fingerprint],
        )?;
    }
    Ok(gone.len())
}

/// The full fingerprint that `prefix` names.
fn expand(conn: &Connection, prefix: &str) -> Result<String, Error> {
    let mut stmt = conn.prepare(
        "SELECT fingerprint FROM findings WHERE substr(fingerprint, 1, length(?1)) = ?1 LIMIT 2",
    )?;
    let mut found: Vec<String> = stmt
        .query_map(params![prefix], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    match (found.pop(), found.pop()) {
        (Some(only), None) => Ok(only),
        (None, _) => Err(Error::UnknownFinding(prefix.to_owned())),
        _ => Err(Error::AmbiguousFinding(prefix.to_owned())),
    }
}

fn load_finding(conn: &Connection, prefix: &str) -> Result<FindingRecord, Error> {
    let full = expand(conn, prefix)?;
    let sql = format!("SELECT {FINDING_COLUMNS} FROM finding_states WHERE fingerprint = ?1");
    Ok(conn.query_row(&sql, params![full], finding_record)?)
}

fn load_event(conn: &Connection, id: i64) -> Result<ReviewEvent, Error> {
    let sql = format!("SELECT {EVENT_COLUMNS} FROM review_events WHERE id = ?1");
    Ok(conn.query_row(&sql, params![id], review_event)?)
}

fn append(tx: &Transaction, review: &NewReview, finding: &FindingRecord) -> Result<i64, Error> {
    let snapshot = json!({
        "severity": finding.severity,
        "message": finding.message,
        "path": finding.path,
        "locator": finding.locator,
        "symbol": finding.symbol,
        "evidence": finding.evidence,
        "facts": finding.facts,
        "resolved": finding.resolved_at.is_some(),
        "reopened": finding.reopened,
    });
    let language = finding.facts.get("language").and_then(Value::as_str);
    tx.execute(
        &format!(
            "INSERT INTO review_events (fingerprint, rule_id, rule_version, catalog_version, \
                 pattern_fingerprint, verdict, reason_code, reason_text, reviewer_kind, \
                 reviewer_id, language, scope, evidence, feature_snapshot, git_commit, timestamp) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, {NOW})"
        ),
        params![
            finding.fingerprint,
            finding.rule_id,
            review.rule_version,
            review.catalog_version,
            review.pattern_fingerprint,
            review.verdict.as_str(),
            review.reason.as_str(),
            review.reason_text,
            review.reviewer_kind.as_str(),
            review.reviewer_id,
            language,
            review.scope,
            finding.evidence.to_string(),
            snapshot.to_string(),
            review.commit,
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

fn finding_record(row: &Row) -> rusqlite::Result<FindingRecord> {
    let verdict: Option<String> = row.get(13)?;
    let reason: Option<String> = row.get(14)?;
    let review = match (verdict, reason) {
        (Some(verdict), Some(reason)) => Some(LatestReview {
            verdict: parse(13, &verdict)?,
            reason: parse(14, &reason)?,
        }),
        _ => None,
    };
    Ok(FindingRecord {
        fingerprint: row.get(0)?,
        rule_id: row.get(1)?,
        severity: row.get(2)?,
        path: row.get(3)?,
        locator: json_column(row, 4)?,
        symbol: row.get(5)?,
        first_seen: row.get(6)?,
        last_seen: row.get(7)?,
        resolved_at: row.get(8)?,
        reopened: row.get(9)?,
        message: row.get(10)?,
        evidence: json_column(row, 11)?,
        facts: json_column(row, 12)?,
        review,
        suppressed: row.get(15)?,
    })
}

fn review_event(row: &Row) -> rusqlite::Result<ReviewEvent> {
    Ok(ReviewEvent {
        id: row.get(0)?,
        fingerprint: row.get(1)?,
        rule_id: row.get(2)?,
        rule_version: row.get(3)?,
        catalog_version: row.get(4)?,
        pattern_fingerprint: row.get(5)?,
        verdict: parse(6, &row.get::<_, String>(6)?)?,
        reason: parse(7, &row.get::<_, String>(7)?)?,
        reason_text: row.get(8)?,
        reviewer_kind: parse(9, &row.get::<_, String>(9)?)?,
        reviewer_id: row.get(10)?,
        language: row.get(11)?,
        scope: row.get(12)?,
        evidence: json_column(row, 13)?,
        feature_snapshot: json_column(row, 14)?,
        commit: row.get(15)?,
        timestamp: row.get(16)?,
    })
}

fn json_column(row: &Row, index: usize) -> rusqlite::Result<Value> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text).map_err(|e| conversion(index, e))
}

fn parse<T: FromStr>(index: usize, text: &str) -> rusqlite::Result<T>
where
    T::Err: StdError + Send + Sync + 'static,
{
    text.parse().map_err(|e| conversion(index, e))
}

fn conversion(index: usize, error: impl StdError + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
}

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}
