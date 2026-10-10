use rusqlite::{Connection, TransactionBehavior};

use crate::Error;

/// The schema version this build writes (`PRAGMA user_version`). The database
/// is a cache of the decision log and of what runs saw, so it has no migration
/// chain: one with another version is dropped and built again, and the
/// judgments come back from `decisions.jsonl`.
pub(crate) const VERSION: i64 = 8;

/// Findings with the judgments and suppressions on them. Locators, evidence and
/// facts are JSON text, so nothing here assumes the artifact is code.
///
/// `judgments` and `suppressions` are append-only, enforced by triggers: a
/// judgment is a recorded label, and a later one replaces it in the views
/// without editing it. The latest judgment of a finding stands; it hides the
/// finding when it is `pass` or `notApplicable`, or `fail` with an accepted
/// suppression, and stops doing so when the decision's meaning or the
/// finding's evidence moved. What the decision authored as `error` is never
/// hidden by a judgment.
const SCHEMA: &str = "
CREATE TABLE findings (
    fingerprint       TEXT PRIMARY KEY,
    rule_id           TEXT NOT NULL,
    decision_uid      TEXT,
    severity          TEXT NOT NULL CHECK (severity IN ('error', 'warn', 'info')),
    authored_severity TEXT NOT NULL CHECK (authored_severity IN ('error', 'warn', 'info')),
    path              TEXT NOT NULL,
    locator           TEXT NOT NULL,
    symbol            TEXT,
    first_seen        TEXT NOT NULL,
    last_seen         TEXT NOT NULL,
    resolved_at       TEXT,
    inactive_at       TEXT,
    reopened          INTEGER NOT NULL DEFAULT 0,
    last_message      TEXT NOT NULL,
    last_evidence     TEXT NOT NULL,
    last_facts        TEXT NOT NULL,
    last_options      TEXT NOT NULL DEFAULT '{}',
    last_commit       TEXT,
    last_dirty        INTEGER,
    lighthouse_version TEXT,
    catalog_version   TEXT,
    meaning_version   TEXT,
    check_revision    TEXT,
    decision_hash     TEXT,
    evidence_digest   TEXT
);
CREATE INDEX findings_rule ON findings (rule_id);
CREATE INDEX findings_path ON findings (path);

CREATE TABLE judgments (
    judgment_id        TEXT PRIMARY KEY,
    fingerprint        TEXT NOT NULL,
    rule_id            TEXT NOT NULL,
    decision_uid       TEXT,
    meaning_version    TEXT,
    check_revision     TEXT,
    decision_hash      TEXT,
    catalog_version    TEXT,
    lighthouse_version TEXT,
    judgment           TEXT NOT NULL CHECK (judgment IN ('pass', 'fail', 'notApplicable')),
    reason             TEXT,
    agent_type         TEXT NOT NULL CHECK (agent_type IN ('Person', 'SoftwareAgent')),
    agent_id           TEXT,
    language           TEXT,
    scope              TEXT,
    evidence_digest    TEXT,
    feature_snapshot   TEXT NOT NULL,
    git_commit         TEXT,
    generated_at       TEXT NOT NULL
);
CREATE INDEX judgments_fingerprint ON judgments (fingerprint, generated_at);
CREATE INDEX judgments_rule ON judgments (rule_id);
CREATE TRIGGER judgments_append_only_update BEFORE UPDATE ON judgments
BEGIN SELECT RAISE(ABORT, 'judgments is append-only'); END;
CREATE TRIGGER judgments_append_only_delete BEFORE DELETE ON judgments
BEGIN SELECT RAISE(ABORT, 'judgments is append-only'); END;

CREATE TABLE suppressions (
    suppression_id TEXT PRIMARY KEY,
    judgment_id    TEXT NOT NULL,
    fingerprint    TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN ('inSource', 'external')),
    status         TEXT NOT NULL CHECK (status IN ('accepted', 'underReview', 'rejected')),
    justification  TEXT NOT NULL,
    agent_type     TEXT NOT NULL CHECK (agent_type IN ('Person', 'SoftwareAgent')),
    agent_id       TEXT,
    generated_at   TEXT NOT NULL
);
CREATE INDEX suppressions_judgment ON suppressions (judgment_id);
CREATE TRIGGER suppressions_append_only_update BEFORE UPDATE ON suppressions
BEGIN SELECT RAISE(ABORT, 'suppressions is append-only'); END;
CREATE TRIGGER suppressions_append_only_delete BEFORE DELETE ON suppressions
BEGIN SELECT RAISE(ABORT, 'suppressions is append-only'); END;

CREATE TABLE fix_events (
    event_id           TEXT PRIMARY KEY,
    fingerprint        TEXT NOT NULL,
    rule_id            TEXT NOT NULL,
    fixer              TEXT NOT NULL,
    safety             TEXT NOT NULL CHECK (safety IN ('safe', 'suggested')),
    description        TEXT NOT NULL,
    files              TEXT NOT NULL,
    git_commit         TEXT,
    lighthouse_version TEXT,
    timestamp          TEXT NOT NULL
);
CREATE INDEX fix_events_fingerprint ON fix_events (fingerprint, timestamp);

CREATE VIEW latest_judgments AS
SELECT j.fingerprint, j.judgment_id, j.judgment, j.reason, j.meaning_version, j.evidence_digest
FROM judgments j
WHERE j.judgment_id = (
    SELECT x.judgment_id FROM judgments x WHERE x.fingerprint = j.fingerprint
    ORDER BY x.generated_at DESC, x.judgment_id DESC LIMIT 1
);

CREATE VIEW standings AS
SELECT fingerprint, judgment, justification, narrowing,
    CASE
        WHEN hides AND authored = 'error' THEN 'unsuppressible'
        WHEN judged_version IS NOT NULL AND meaning_version IS NOT NULL
             AND judged_version <> meaning_version THEN 'rule-changed'
        WHEN judged_digest IS NOT NULL AND evidence_digest IS NOT NULL
             AND judged_digest <> evidence_digest THEN 'evidence-changed'
        WHEN hides THEN 'suppressed'
        ELSE 'judged'
    END AS standing
FROM (
    SELECT f.fingerprint, l.judgment, f.meaning_version, f.evidence_digest,
        l.meaning_version AS judged_version, l.evidence_digest AS judged_digest,
        f.authored_severity AS authored,
        (l.judgment = 'notApplicable') AS narrowing,
        (SELECT s.justification FROM suppressions s
         WHERE s.judgment_id = l.judgment_id AND s.status = 'accepted'
         ORDER BY s.generated_at, s.suppression_id LIMIT 1) AS justification,
        (l.judgment <> 'fail' OR EXISTS (
            SELECT 1 FROM suppressions s
            WHERE s.judgment_id = l.judgment_id AND s.status = 'accepted')) AS hides
    FROM findings f
    JOIN latest_judgments l ON l.fingerprint = f.fingerprint
);

CREATE VIEW finding_states AS
SELECT f.*, l.judgment AS latest_judgment,
       s.standing AS standing, s.justification AS justification,
       COALESCE(s.narrowing, 0) AS narrowing
FROM findings f
LEFT JOIN latest_judgments l ON l.fingerprint = f.fingerprint
LEFT JOIN standings s ON s.fingerprint = f.fingerprint;
";

/// Brings the database to this build's schema. The version is read after
/// taking the write lock, so two processes opening a fresh database take
/// turns and the second finds nothing to do. A database of another version
/// is emptied and built again in the same transaction.
pub(crate) fn prepare(conn: &mut Connection) -> Result<(), Error> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let found: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if found != VERSION {
        if found != 0 {
            drop_all(&tx)?;
        }
        tx.execute_batch(SCHEMA)?;
        tx.execute_batch(&format!("PRAGMA user_version = {VERSION}"))?;
    }
    tx.commit()?;
    Ok(())
}

/// The version recorded in the database.
pub(crate) fn version(conn: &Connection) -> Result<i64, Error> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Drops every table, view and trigger.
fn drop_all(conn: &Connection) -> Result<(), Error> {
    let mut stmt = conn.prepare(
        "SELECT type, name FROM sqlite_master \
         WHERE type IN ('trigger', 'view', 'table') AND name NOT LIKE 'sqlite_%' \
         ORDER BY CASE type WHEN 'trigger' THEN 0 WHEN 'view' THEN 1 ELSE 2 END",
    )?;
    let objects: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (kind, name) in objects {
        conn.execute_batch(&format!("DROP {kind} IF EXISTS \"{name}\""))?;
    }
    Ok(())
}
