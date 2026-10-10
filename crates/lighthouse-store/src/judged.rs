//! What the judgments on a finding do to it now. The judgment that stands is
//! picked from the records of the decision log (their copy in the cache), and
//! compared with the finding as the current run saw it, so a clone that holds
//! only the log applies a committed judgment the same as the machine that
//! recorded it.

use std::collections::BTreeMap;

use lighthouse_model::{AgentKind, Judgment, Severity};
use rusqlite::Connection;

use crate::{Error, Ruling, Standing, Subject};

/// A judgment as the ranking sees it.
struct Candidate {
    id: String,
    judgment: Judgment,
    meaning_version: Option<String>,
    evidence_digest: Option<String>,
    strength: u8,
    generated_at: String,
    /// An accepted suppression goes with it.
    suppressed: bool,
    justification: Option<String>,
}

/// Every judgment of the cache, by fingerprint.
pub(crate) struct Judged(BTreeMap<String, Vec<Candidate>>);

impl Judged {
    pub(crate) fn load(conn: &Connection) -> Result<Self, Error> {
        let mut suppressions: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut stmt = conn.prepare(
            "SELECT judgment_id, justification FROM suppressions WHERE status = 'accepted' \
             ORDER BY generated_at, suppression_id",
        )?;
        for row in stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))? {
            let (judgment, justification): (String, String) = row?;
            let justification = Some(justification).filter(|j| !j.trim().is_empty());
            suppressions.entry(judgment).or_insert(justification);
        }
        let mut found: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
        let mut stmt = conn.prepare(
            "SELECT judgment_id, fingerprint, judgment, meaning_version, evidence_digest, \
                 agent_type, generated_at FROM judgments",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        for row in rows {
            let (id, fingerprint, judgment, meaning_version, evidence_digest, agent, at) = row?;
            let accepted = suppressions.get(&id);
            let candidate = Candidate {
                judgment: judgment.parse().map_err(invalid)?,
                strength: agent.parse::<AgentKind>().map_err(invalid)?.strength(),
                suppressed: accepted.is_some(),
                justification: accepted.cloned().flatten(),
                id,
                meaning_version,
                evidence_digest,
                generated_at: at,
            };
            found.entry(fingerprint).or_default().push(candidate);
        }
        Ok(Self(found))
    }

    /// What the judgment that stands does to `subject`, if one does.
    ///
    /// Among the judgments given under the subject's meaning version (a record
    /// that names none matches any), the stronger attribution stands, a person
    /// over an agent; at equal strength the later `generatedAtTime`, then the
    /// larger id. A weaker judgment is recorded and does not stand. When no
    /// judgment was given under the current meaning version, the same order
    /// picks among all of them, and the one that stands has expired.
    pub(crate) fn ruling(&self, subject: &Subject) -> Option<Ruling> {
        let all = self.0.get(&subject.fingerprint)?;
        let same: Vec<&Candidate> = all
            .iter()
            .filter(|c| {
                agree(
                    c.meaning_version.as_deref(),
                    subject.meaning_version.as_deref(),
                )
            })
            .collect();
        let pool: Vec<&Candidate> = if same.is_empty() {
            all.iter().collect()
        } else {
            same
        };
        let top = pool.into_iter().max_by(|a, b| {
            (a.strength, &a.generated_at, &a.id).cmp(&(b.strength, &b.generated_at, &b.id))
        })?;
        let hides = top.judgment != Judgment::Fail || top.suppressed;
        let standing = if hides && subject.authored_severity == Severity::Error {
            Standing::Unsuppressible
        } else if !agree(
            top.meaning_version.as_deref(),
            subject.meaning_version.as_deref(),
        ) {
            Standing::RuleChanged
        } else if !agree(
            top.evidence_digest.as_deref(),
            Some(subject.evidence_digest.as_str()),
        ) {
            Standing::EvidenceChanged
        } else if hides {
            Standing::Suppressed
        } else {
            Standing::Judged
        };
        Some(Ruling {
            standing,
            judgment: top.judgment,
            justification: top.justification.clone(),
        })
    }
}

/// Whether a recorded version or digest matches the current one; a record
/// that holds none matches anything.
fn agree(recorded: Option<&str>, current: Option<&str>) -> bool {
    match (recorded, current) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

fn invalid(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}
