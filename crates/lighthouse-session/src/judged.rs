//! The judgments of a project, as the evaluation and training of a model read
//! them.

use std::path::Path;

use lighthouse_ml::labels::Judged;
use lighthouse_store::Records;

use crate::Result;

/// Every judgment the project's log holds.
pub(crate) fn judged(root: &Path) -> Result<Vec<Judged>> {
    Ok(Records::read(root)?
        .judgments
        .into_iter()
        .map(|event| Judged {
            fingerprint: event.spec.fingerprint,
            decision_name: event.spec.decision,
            decision_uid: event.spec.decision_uid,
            meaning_version: event.spec.meaning_version,
            judgment: event.spec.judgment,
            by: event.spec.was_attributed_to,
        })
        .collect())
}
