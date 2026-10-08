use rusqlite::{Connection, TransactionBehavior};

use crate::Error;

/// Schema migrations, oldest first; `PRAGMA user_version` holds how many have
/// been applied. A migration never changes once released: a change is a new
/// entry.
///
/// `review_events` is append-only, enforced by triggers, so a migration that
/// changes its shape rewrites it: in the one transaction every migration runs
/// in, it drops the triggers and the views over the table, creates the new
/// table, copies every row across, drops the old table, renames the new one,
/// and recreates indexes, triggers and views. `V2` does exactly that.
const MIGRATIONS: &[&str] = &[V1, V2, V3, V4, V5];

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

/// The decision log becomes the source of truth for verdicts, and the table a
/// cache of it: events are identified by content (`event_id`), ordered by
/// time, and no longer reference findings, since a teammate's verdict may
/// arrive before this machine has seen the finding. A verdict now carries the
/// semantic version of its rule and a digest of its evidence, and stops
/// suppressing when either moves; mechanical findings are never suppressed.
/// Findings keep the sighting a review will snapshot, and can go inactive.
///
/// Rows from `V1` keep a `legacy-<n>` id and have no semantic version or
/// digest, which a verdict treats as matching anything.
const V2: &str = "
DROP VIEW finding_states;
DROP VIEW suppressions;
DROP VIEW latest_verdicts;
DROP TRIGGER review_events_append_only_update;
DROP TRIGGER review_events_append_only_delete;

ALTER TABLE findings RENAME COLUMN severity TO last_severity;
ALTER TABLE findings ADD COLUMN tier TEXT;
ALTER TABLE findings ADD COLUMN last_options TEXT NOT NULL DEFAULT '{}';
ALTER TABLE findings ADD COLUMN last_commit TEXT;
ALTER TABLE findings ADD COLUMN last_dirty INTEGER;
ALTER TABLE findings ADD COLUMN lighthouse_version TEXT;
ALTER TABLE findings ADD COLUMN catalog_version TEXT;
ALTER TABLE findings ADD COLUMN rule_version TEXT;
ALTER TABLE findings ADD COLUMN pattern_hash TEXT;
ALTER TABLE findings ADD COLUMN evidence_digest TEXT;
ALTER TABLE findings ADD COLUMN inactive_at TEXT;

CREATE TABLE review_events_v2 (
    event_id            TEXT PRIMARY KEY,
    fingerprint         TEXT NOT NULL,
    rule_id             TEXT NOT NULL,
    rule_version        TEXT,
    pattern_hash        TEXT,
    catalog_version     TEXT,
    lighthouse_version  TEXT,
    pattern_fingerprint TEXT,
    verdict             TEXT NOT NULL CHECK (verdict IN ('confirmed', 'rejected', 'deferred')),
    reason_code         TEXT NOT NULL,
    reason_text         TEXT,
    reviewer_kind       TEXT NOT NULL CHECK (reviewer_kind IN ('agent', 'human')),
    reviewer_id         TEXT,
    language            TEXT,
    scope               TEXT,
    evidence_digest     TEXT,
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
INSERT INTO review_events_v2 (event_id, fingerprint, rule_id, pattern_hash, catalog_version,
    pattern_fingerprint, verdict, reason_code, reason_text, reviewer_kind, reviewer_id, language,
    scope, feature_snapshot, git_commit, timestamp)
SELECT 'legacy-' || id, fingerprint, rule_id, rule_version, catalog_version,
    pattern_fingerprint, verdict, reason_code, reason_text, reviewer_kind, reviewer_id, language,
    scope, feature_snapshot, git_commit, timestamp
FROM review_events;
DROP TABLE review_events;
ALTER TABLE review_events_v2 RENAME TO review_events;

CREATE INDEX review_events_fingerprint ON review_events (fingerprint, timestamp);
CREATE INDEX review_events_rule ON review_events (rule_id);
CREATE TRIGGER review_events_append_only_update BEFORE UPDATE ON review_events
BEGIN SELECT RAISE(ABORT, 'review_events is append-only'); END;
CREATE TRIGGER review_events_append_only_delete BEFORE DELETE ON review_events
BEGIN SELECT RAISE(ABORT, 'review_events is append-only'); END;

CREATE VIEW latest_verdicts AS
SELECT e.fingerprint, e.event_id, e.verdict, e.reason_code, e.rule_version, e.evidence_digest
FROM review_events e
WHERE e.event_id = (
    SELECT x.event_id FROM review_events x WHERE x.fingerprint = e.fingerprint
    ORDER BY x.timestamp DESC, x.event_id DESC LIMIT 1
);

CREATE VIEW standings AS
SELECT f.fingerprint, l.reason_code, (l.reason_code = 'scope-too-broad') AS narrowing,
    CASE
        WHEN f.last_severity = 'error' THEN 'unsuppressible'
        WHEN l.rule_version IS NOT NULL AND f.rule_version IS NOT NULL
             AND l.rule_version <> f.rule_version THEN 'rule-changed'
        WHEN l.evidence_digest IS NOT NULL AND f.evidence_digest IS NOT NULL
             AND l.evidence_digest <> f.evidence_digest THEN 'evidence-changed'
        ELSE 'suppressed'
    END AS standing
FROM findings f
JOIN latest_verdicts l ON l.fingerprint = f.fingerprint
WHERE l.verdict = 'rejected';

CREATE VIEW finding_states AS
SELECT f.*, l.verdict AS review_verdict, l.reason_code AS review_reason,
       s.standing AS standing, COALESCE(s.narrowing, 0) AS narrowing
FROM findings f
LEFT JOIN latest_verdicts l ON l.fingerprint = f.fingerprint
LEFT JOIN standings s ON s.fingerprint = f.fingerprint;
";

/// Fixes the orchestrator applied: which fixer changed the code for which
/// finding. A record of what happened to this checkout, kept in the local
/// cache next to the sightings; it is not a decision, so it stays out of the
/// shared decision log.
const V3: &str = "
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
";

/// The resource model renames a pattern to a decision, so `pattern_hash`
/// becomes `decision_hash` in both tables, and gives a finding the semantic
/// version its rule had before the model (`legacy_rule_version`): a verdict
/// recorded then keeps applying while the decision demands the same. The
/// views that compare versions are recreated to honor it. No row is touched.
const V4: &str = "
ALTER TABLE findings RENAME COLUMN pattern_hash TO decision_hash;
ALTER TABLE review_events RENAME COLUMN pattern_hash TO decision_hash;
ALTER TABLE findings ADD COLUMN legacy_rule_version TEXT;

DROP VIEW finding_states;
DROP VIEW standings;

CREATE VIEW standings AS
SELECT f.fingerprint, l.reason_code, (l.reason_code = 'scope-too-broad') AS narrowing,
    CASE
        WHEN f.last_severity = 'error' THEN 'unsuppressible'
        WHEN l.rule_version IS NOT NULL AND f.rule_version IS NOT NULL
             AND l.rule_version <> f.rule_version
             AND l.rule_version IS NOT f.legacy_rule_version THEN 'rule-changed'
        WHEN l.evidence_digest IS NOT NULL AND f.evidence_digest IS NOT NULL
             AND l.evidence_digest <> f.evidence_digest THEN 'evidence-changed'
        ELSE 'suppressed'
    END AS standing
FROM findings f
JOIN latest_verdicts l ON l.fingerprint = f.fingerprint
WHERE l.verdict = 'rejected';

CREATE VIEW finding_states AS
SELECT f.*, l.verdict AS review_verdict, l.reason_code AS review_reason,
       s.standing AS standing, COALESCE(s.narrowing, 0) AS narrowing
FROM findings f
LEFT JOIN latest_verdicts l ON l.fingerprint = f.fingerprint
LEFT JOIN standings s ON s.fingerprint = f.fingerprint;
";

/// A verdict can suppress a finding of a heuristic or judgment decision,
/// whatever severity the project gave it, and never one of a mechanical
/// decision: the tier decides, not the severity. A finding recorded before
/// tiers existed has none, and then the severity it had stands in for it, as
/// it did. The views are recreated; no row is touched.
const V5: &str = "
DROP VIEW finding_states;
DROP VIEW standings;

CREATE VIEW standings AS
SELECT f.fingerprint, l.reason_code, (l.reason_code = 'scope-too-broad') AS narrowing,
    CASE
        WHEN COALESCE(f.tier, CASE WHEN f.last_severity = 'error' THEN 'mechanical' ELSE 'heuristic' END)
             NOT IN ('heuristic', 'judgment') THEN 'unsuppressible'
        WHEN l.rule_version IS NOT NULL AND f.rule_version IS NOT NULL
             AND l.rule_version <> f.rule_version
             AND l.rule_version IS NOT f.legacy_rule_version THEN 'rule-changed'
        WHEN l.evidence_digest IS NOT NULL AND f.evidence_digest IS NOT NULL
             AND l.evidence_digest <> f.evidence_digest THEN 'evidence-changed'
        ELSE 'suppressed'
    END AS standing
FROM findings f
JOIN latest_verdicts l ON l.fingerprint = f.fingerprint
WHERE l.verdict = 'rejected';

CREATE VIEW finding_states AS
SELECT f.*, l.verdict AS review_verdict, l.reason_code AS review_reason,
       s.standing AS standing, COALESCE(s.narrowing, 0) AS narrowing
FROM findings f
LEFT JOIN latest_verdicts l ON l.fingerprint = f.fingerprint
LEFT JOIN standings s ON s.fingerprint = f.fingerprint;
";

/// The schema version this build writes.
pub(crate) fn current() -> usize {
    MIGRATIONS.len()
}

/// Brings the database up to date. The version is read again after taking
/// the write lock, so two processes opening a fresh database take turns and
/// the second finds nothing to do; a database written by a newer build is
/// refused rather than guessed at.
pub(crate) fn migrate(conn: &mut Connection) -> Result<(), Error> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let found = version(&tx)?;
    if found > current() {
        return Err(Error::NewerSchema {
            found,
            supported: current(),
        });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(found) {
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", index + 1))?;
    }
    tx.commit()?;
    Ok(())
}

/// How many migrations the database has applied.
pub(crate) fn version(conn: &Connection) -> Result<usize, Error> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(usize::try_from(version).unwrap_or(usize::MAX))
}
