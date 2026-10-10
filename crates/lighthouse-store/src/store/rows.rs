//! Reading and writing the rows of the store: findings, judgments and
//! suppressions, and the errors that come with the database.

use std::{collections::BTreeMap, path::Path};

use lighthouse_model::{Attribution, Judgment, Severity};
use rusqlite::{
    Connection, OptionalExtension, Row, Transaction, named_params, params, types::Type,
};
use serde_json::{Value, json};

use crate::{
    Error, FindingRecord, JudgmentEvent, NewJudgment, Stamp, Standing, StatusFilter, Subject,
    SuppressionEvent, digest, judged::Judged, log, log::LoggedSuppression,
};
use lighthouse_model::{SuppressionKind, SuppressionStatus};

use super::{JUDGMENT_COLUMNS, MAX_CANDIDATES, SNAPSHOT_VERSION};

/// The full fingerprint that `prefix` names, among findings and judgments.
pub(super) fn expand(conn: &Connection, prefix: &str) -> Result<String, Error> {
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

pub(super) fn load_finding(conn: &Connection, prefix: &str) -> Result<FindingRecord, Error> {
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
pub(super) fn listed(finding: &FindingRecord, status: StatusFilter) -> bool {
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
pub(super) fn judgments_of(
    conn: &Connection,
    fingerprint: Option<&str>,
) -> Result<Vec<JudgmentEvent>, Error> {
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

pub(super) fn suppressions_of(conn: &Connection) -> Result<Vec<LoggedSuppression>, Error> {
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
pub(super) fn event_of(
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
pub(super) fn suppression_of(
    event: &JudgmentEvent,
    justification: &str,
) -> Result<LoggedSuppression, Error> {
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

pub(super) fn insert_judgment(tx: &Transaction, event: &JudgmentEvent) -> Result<(), Error> {
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

pub(super) fn insert_suppression(tx: &Transaction, s: &LoggedSuppression) -> Result<(), Error> {
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

pub(super) fn finding_record(row: &Row, judged: &Judged) -> rusqlite::Result<FindingRecord> {
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

pub(super) fn judgment_event(row: &Row) -> rusqlite::Result<JudgmentEvent> {
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

pub(super) fn severity(row: &Row, name: &str) -> rusqlite::Result<Severity> {
    parse(&row.get::<_, String>(name)?)
}

pub(super) fn json_column(row: &Row, name: &str) -> rusqlite::Result<Value> {
    let text: String = row.get(name)?;
    serde_json::from_str(&text).map_err(conversion)
}

pub(super) fn parse<T: std::str::FromStr>(text: &str) -> rusqlite::Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    text.parse().map_err(conversion)
}

pub(super) fn conversion(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(error))
}

pub(super) fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}
