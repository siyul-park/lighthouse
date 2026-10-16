//! What the judgments on a finding do to it now. The judgment that stands is
//! picked from the records of the decision log (their copy in the cache), and
//! compared with the finding as the current run saw it, so a clone that holds
//! only the log applies a committed judgment the same as the machine that
//! recorded it.

use std::collections::BTreeMap;

use lighthouse_model::{Judgment, Severity, SuppressionStatus};

use crate::{JudgmentEvent, Ruling, Standing, Subject};

/// Every judgment of the cache, by fingerprint.
pub(crate) struct Standings(BTreeMap<String, Vec<JudgmentEvent>>);

impl Standings {
    pub(crate) fn of(events: Vec<JudgmentEvent>) -> Self {
        let mut found: BTreeMap<String, Vec<JudgmentEvent>> = BTreeMap::new();
        for event in events {
            found
                .entry(event.spec.fingerprint.clone())
                .or_default()
                .push(event);
        }
        Self(found)
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
        let same: Vec<&JudgmentEvent> = all
            .iter()
            .filter(|e| {
                agree(
                    e.spec.meaning_version.as_deref(),
                    subject.meaning_version.as_deref(),
                )
            })
            .collect();
        let pool: Vec<&JudgmentEvent> = if same.is_empty() {
            all.iter().collect()
        } else {
            same
        };
        let top = pool.into_iter().max_by(|a, b| rank(a).cmp(&rank(b)))?;
        let accepted = top
            .suppressions
            .iter()
            .find(|s| s.spec.status == SuppressionStatus::Accepted);
        let hides = top.spec.judgment != Judgment::Fail || accepted.is_some();
        let standing = if hides && subject.authored_severity == Severity::Error {
            Standing::Unsuppressible
        } else if !agree(
            top.spec.meaning_version.as_deref(),
            subject.meaning_version.as_deref(),
        ) {
            Standing::RuleChanged
        } else if !agree(
            top.spec.evidence_digest.as_deref(),
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
            judgment: top.spec.judgment,
            justification: accepted
                .map(|s| s.spec.justification.clone())
                .filter(|j| !j.trim().is_empty()),
        })
    }
}

/// How strongly a judgment stands: the stronger attribution, then the later
/// `generatedAtTime`, then the larger id.
fn rank(event: &JudgmentEvent) -> (u8, &str, &str) {
    (
        event.spec.was_attributed_to.strength(),
        &event.spec.generated_at_time,
        &event.id,
    )
}

/// Whether a recorded version or digest matches the current one; a record
/// that holds none matches anything.
fn agree(recorded: Option<&str>, current: Option<&str>) -> bool {
    match (recorded, current) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}
