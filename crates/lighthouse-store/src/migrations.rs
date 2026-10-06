use rusqlite::Connection;

use crate::Error;

/// Schema migrations, oldest first; `PRAGMA user_version` holds how many have
/// been applied. A migration never changes once released: a change is a new
/// entry.
const MIGRATIONS: &[&str] = &[V1];

/// Findings and the append-only review log. Locators, evidence and facts are
/// JSON text, so nothing here assumes the artifact is code.
const V1: &str = "
CREATE TABLE findings (
    fingerprint   TEXT PRIMARY KEY,
    rule_id       TEXT NOT NULL,
    severity      TEXT NOT NULL,
    path          TEXT NOT NULL,
    locator       TEXT NOT NULL,
    symbol        TEXT,
    first_seen    TEXT NOT NULL,
    last_seen     TEXT NOT NULL,
    resolved_at   TEXT,
    reopened      INTEGER NOT NULL DEFAULT 0,
    last_message  TEXT NOT NULL,
    last_evidence TEXT NOT NULL,
    last_facts    TEXT NOT NULL
);
CREATE INDEX findings_rule ON findings (rule_id);
CREATE INDEX findings_path ON findings (path);

CREATE TABLE review_events (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    fingerprint         TEXT NOT NULL REFERENCES findings (fingerprint),
    rule_id             TEXT NOT NULL,
    rule_version        TEXT,
    catalog_version     TEXT,
    pattern_fingerprint TEXT,
    verdict             TEXT NOT NULL CHECK (verdict IN ('confirmed', 'rejected', 'deferred')),
    reason_code         TEXT NOT NULL,
    reason_text         TEXT,
    reviewer_kind       TEXT NOT NULL CHECK (reviewer_kind IN ('agent', 'human')),
    reviewer_id         TEXT,
    language            TEXT,
    scope               TEXT,
    evidence            TEXT NOT NULL,
    feature_snapshot    TEXT NOT NULL,
    git_commit          TEXT,
    timestamp           TEXT NOT NULL,
    CHECK (
        (verdict = 'confirmed' AND reason_code IN ('fixed', 'accepted-debt', 'none')) OR
        (verdict = 'rejected' AND reason_code IN ('false-positive', 'intentional-exception',
            'scope-too-broad', 'project-allowed', 'not-worth-fixing')) OR
        (verdict = 'deferred' AND reason_code = 'none')
    )
);
CREATE INDEX review_events_fingerprint ON review_events (fingerprint, id);
CREATE INDEX review_events_rule ON review_events (rule_id);

CREATE TRIGGER review_events_append_only_update BEFORE UPDATE ON review_events
BEGIN SELECT RAISE(ABORT, 'review_events is append-only'); END;
CREATE TRIGGER review_events_append_only_delete BEFORE DELETE ON review_events
BEGIN SELECT RAISE(ABORT, 'review_events is append-only'); END;

CREATE VIEW latest_verdicts AS
SELECT fingerprint, id AS event_id, verdict, reason_code
FROM review_events e
WHERE id = (SELECT MAX(id) FROM review_events WHERE fingerprint = e.fingerprint);

CREATE VIEW suppressions AS
SELECT fingerprint, event_id, reason_code, (reason_code = 'scope-too-broad') AS narrowing
FROM latest_verdicts
WHERE verdict = 'rejected';

CREATE VIEW finding_states AS
SELECT f.*, l.verdict, l.reason_code,
       EXISTS (SELECT 1 FROM suppressions s WHERE s.fingerprint = f.fingerprint) AS suppressed
FROM findings f
LEFT JOIN latest_verdicts l ON l.fingerprint = f.fingerprint;
";

/// The schema version this build writes.
pub(crate) fn current() -> usize {
    MIGRATIONS.len()
}

/// Brings the database up to date; a database written by a newer build is
/// refused rather than guessed at.
pub(crate) fn migrate(conn: &mut Connection) -> Result<(), Error> {
    let found = version(conn)?;
    if found > current() {
        return Err(Error::NewerSchema {
            found,
            supported: current(),
        });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(found) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", index + 1))?;
        tx.commit()?;
    }
    Ok(())
}

/// How many migrations the database has applied.
pub(crate) fn version(conn: &Connection) -> Result<usize, Error> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(usize::try_from(version).unwrap_or(usize::MAX))
}
