//! Reading and writing the rows of the store: findings, judgments and
//! suppressions, and the errors that come with the database.

use std::{collections::BTreeMap, path::Path};

use lighthouse_model::{Attribution, Fingerprint, Judgment, Role, Severity, evidence_digest};
use rusqlite::{
    Connection, OptionalExtension, Row, Transaction, named_params, params, types::Type,
};
use serde_json::{Value, json};

use crate::{
    Error, FindingRecord, JudgmentEvent, JudgmentSpec, NewJudgment, Standing, State, Subject,
    SuppressionEvent, SuppressionSpec, Versions, standings::Standings,
};

use super::{JUDGMENT_COLUMNS, MAX_CANDIDATES, SNAPSHOT_VERSION};

/// The full fingerprint that `prefix` names, among findings and judgments: a
/// whole fingerprint, or a unique prefix of at least [`Fingerprint::MIN_PREFIX`] characters.
pub(super) fn expand(conn: &Connection, prefix: &str) -> Result<String, Error> {
    let whole = conn
        .query_row(
            "SELECT fingerprint FROM findings WHERE fingerprint = ?1 \
             UNION SELECT fingerprint FROM judgments WHERE fingerprint = ?1",
            params![prefix],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(whole) = whole {
        return Ok(whole);
    }
    if prefix.len() < Fingerprint::MIN_PREFIX {
        return Err(Error::ShortFingerprint(prefix.to_owned()));
    }
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
    let standings = standings(conn)?;
    conn.query_row(
        "SELECT * FROM findings WHERE fingerprint = ?1",
        params![full],
        |row| finding_record(row, &standings),
    )
    .optional()?
    .ok_or_else(|| Error::UnknownFinding(prefix.to_owned()))
}

/// What every judgment of the cache does to its finding.
pub(super) fn standings(conn: &Connection) -> Result<Standings, Error> {
    Ok(Standings::of(judgments_of(conn, None)?))
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
    let mut stmt = conn.prepare(
        "SELECT suppression_id, judgment_id, fingerprint, kind, status, justification, \
             agent_type, agent_id, agent_role, generated_at FROM suppressions \
         WHERE (?1 IS NULL OR fingerprint = ?1) ORDER BY generated_at, suppression_id",
    )?;
    let rows = stmt.query_map(params![fingerprint], |row| {
        Ok(SuppressionEvent {
            id: row.get(0)?,
            spec: SuppressionSpec {
                judgment: row.get(1)?,
                fingerprint: row.get(2)?,
                kind: parse(&row.get::<_, String>(3)?)?,
                status: parse(&row.get::<_, String>(4)?)?,
                justification: row.get(5)?,
                was_attributed_to: Attribution {
                    kind: parse(&row.get::<_, String>(6)?)?,
                    id: row.get(7)?,
                    role: role_of(row.get(8)?)?,
                },
                generated_at_time: row.get(9)?,
            },
        })
    })?;
    let mut by_judgment: BTreeMap<String, Vec<SuppressionEvent>> = BTreeMap::new();
    for suppression in rows {
        let suppression = suppression?;
        by_judgment
            .entry(suppression.spec.judgment.clone())
            .or_default()
            .push(suppression);
    }
    for event in &mut events {
        event.suppressions = by_judgment.remove(&event.id).unwrap_or_default();
    }
    Ok(events)
}

/// The judgment that recording `review` on `finding` writes: the finding
/// frozen as it was last seen, with the id its content determines.
pub(super) fn event_of(
    review: &NewJudgment,
    finding: &FindingRecord,
    versions: Versions,
    generated_at_time: String,
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
    JudgmentEvent::new(JudgmentSpec {
        fingerprint: finding.fingerprint.clone(),
        decision: finding.decision.clone(),
        decision_uid: versions
            .decision_uid
            .or_else(|| finding.decision_uid.clone()),
        meaning_version: versions.meaning_version,
        check_revision: versions.check_revision,
        decision_hash: versions.decision_hash,
        catalog_version: versions.catalog_version,
        lighthouse_version: Some(review.lighthouse_version.clone()),
        judgment: review.judgment,
        reason: review.reason.clone(),
        was_attributed_to: review.attribution.clone(),
        generated_at_time,
        language: finding
            .facts
            .get("language")
            .and_then(Value::as_str)
            .map(str::to_owned),
        scope: versions.scope,
        evidence_digest: Some(evidence_digest(&finding.evidence)),
        snapshot,
        commit: review.commit.clone(),
    })
}

/// Writes the judgment and the suppressions that go with it.
pub(super) fn insert_judgment(tx: &Transaction, event: &JudgmentEvent) -> Result<(), Error> {
    let spec = &event.spec;
    tx.execute(
        "INSERT OR IGNORE INTO judgments (judgment_id, fingerprint, decision, decision_uid, \
             meaning_version, check_revision, decision_hash, catalog_version, lighthouse_version, \
             judgment, reason, agent_type, agent_id, agent_role, language, scope, evidence_digest, \
             feature_snapshot, git_commit, generated_at) \
         VALUES (:id, :fingerprint, :decision, :decision_uid, :meaning_version, :check_revision, \
             :decision_hash, :catalog, :version, :judgment, :reason, :agent_type, :agent_id, :agent_role, \
             :language, :scope, :digest, :snapshot, :commit, :generated_at)",
        named_params! {
            ":id": event.id,
            ":fingerprint": spec.fingerprint,
            ":decision": spec.decision,
            ":decision_uid": spec.decision_uid,
            ":meaning_version": spec.meaning_version,
            ":check_revision": spec.check_revision,
            ":decision_hash": spec.decision_hash,
            ":catalog": spec.catalog_version,
            ":version": spec.lighthouse_version,
            ":judgment": spec.judgment.as_str(),
            ":reason": spec.reason,
            ":agent_type": spec.was_attributed_to.kind.as_str(),
            ":agent_id": spec.was_attributed_to.id,
            ":agent_role": spec.was_attributed_to.role.map(Role::as_str),
            ":language": spec.language,
            ":scope": spec.scope,
            ":digest": spec.evidence_digest,
            ":snapshot": spec.snapshot.to_string(),
            ":commit": spec.commit,
            ":generated_at": spec.generated_at_time,
        },
    )?;
    for SuppressionEvent { id, spec: s } in &event.suppressions {
        tx.execute(
            "INSERT OR IGNORE INTO suppressions (suppression_id, judgment_id, fingerprint, kind, \
                 status, justification, agent_type, agent_id, agent_role, generated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                s.judgment,
                s.fingerprint,
                s.kind.as_str(),
                s.status.as_str(),
                s.justification,
                s.was_attributed_to.kind.as_str(),
                s.was_attributed_to.id,
                s.was_attributed_to.role.map(Role::as_str),
                s.generated_at_time,
            ],
        )?;
    }
    Ok(())
}

pub(super) fn finding_record(row: &Row, standings: &Standings) -> rusqlite::Result<FindingRecord> {
    let authored_severity = severity(row, "authored_severity")?;
    let subject = Subject {
        fingerprint: row.get("fingerprint")?,
        authored_severity,
        meaning_version: row.get("meaning_version")?,
        evidence_digest: row
            .get::<_, Option<String>>("evidence_digest")?
            .unwrap_or_default(),
    };
    let ruling = standings.ruling(&subject);
    let standing = ruling.as_ref().map(|r| r.standing);
    let resolved_at: Option<String> = row.get("resolved_at")?;
    let inactive_at: Option<String> = row.get("inactive_at")?;
    let state = if standing == Some(Standing::Suppressed) {
        State::Suppressed
    } else if inactive_at.is_some() {
        State::Inactive
    } else if resolved_at.is_some() {
        State::Resolved
    } else {
        State::Open
    };
    Ok(FindingRecord {
        fingerprint: subject.fingerprint.clone(),
        decision: row.get("decision")?,
        decision_uid: row.get("decision_uid")?,
        severity: severity(row, "severity")?,
        authored_severity,
        path: row.get("path")?,
        locator: json_column(row, "locator")?,
        symbol: row.get("symbol")?,
        first_seen: row.get("first_seen")?,
        last_seen: row.get("last_seen")?,
        resolved_at,
        inactive_at,
        state,
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
        standing,
        justification: ruling.as_ref().and_then(|r| r.justification.clone()),
        narrowing: ruling
            .as_ref()
            .is_some_and(|r| r.judgment == Judgment::NotApplicable),
    })
}

pub(super) fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.display().to_string(),
        source,
    }
}

fn judgment_event(row: &Row) -> rusqlite::Result<JudgmentEvent> {
    Ok(JudgmentEvent {
        id: row.get("judgment_id")?,
        spec: JudgmentSpec {
            fingerprint: row.get("fingerprint")?,
            decision: row.get("decision")?,
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
                role: role_of(row.get("agent_role")?)?,
            },
            generated_at_time: row.get("generated_at")?,
            language: row.get("language")?,
            scope: row.get("scope")?,
            evidence_digest: row.get("evidence_digest")?,
            snapshot: json_column(row, "feature_snapshot")?,
            commit: row.get("git_commit")?,
        },
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

fn role_of(text: Option<String>) -> rusqlite::Result<Option<Role>> {
    text.map(|text| parse(&text)).transpose()
}
