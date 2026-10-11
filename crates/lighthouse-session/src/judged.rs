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
            fingerprint: event.fingerprint,
            decision_name: event.decision_name,
            decision_uid: event.decision_uid,
            meaning_version: event.meaning_version,
            judgment: event.judgment,
            by: event.was_attributed_to,
        })
        .collect())
}
