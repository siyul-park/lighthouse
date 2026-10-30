//! The trained models of the store: one document per model, named by the
//! digest of its canonical form.

use std::{fs, path::Path};

use lighthouse_model::Result;
use lighthouse_plugin::{FineTuningJob, MODELS_DIR, artifact_path};
use lighthouse_protocol::hash::Digest;
use serde_json::Value;

/// The artifact `hex` names in the model store of `root`; `None` when it is not
/// there.
pub(crate) fn read(root: &Path, hex: &str) -> Result<Option<Value>> {
    let path = artifact_path(&root.join(MODELS_DIR), hex);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(None);
    };
    let document = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(document))
}

/// The digest of the artifact a job made.
pub(crate) fn made(job: &FineTuningJob) -> Option<Digest> {
    Digest::parse(job.result_files.first()?)
}
