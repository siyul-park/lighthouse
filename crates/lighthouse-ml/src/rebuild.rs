//! `lighthouse model rebuild`, the host's half: whether the artifact of an
//! attached model is on this machine, and training it again from the dataset
//! it learned from when it is not. A model that learns from labels is a file
//! named by its digest and made by a deterministic trainer, so the same
//! dataset makes the same file.

use std::{fs, path::Path};

use lighthouse_model::hash::digest;
use lighthouse_plugin::{Classifier, FineTuningJobStatus, MODELS_DIR};
use serde::Serialize;
use serde_json::Value;

use crate::{Result, dataset, train::job};

/// How a rebuild went for one attached model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// The artifact is here and is what its name says.
    Present,
    /// The artifact was missing and training made it again, bit for bit.
    Rebuilt,
    /// A file is here, or training made one, that is not what the name says.
    Differs,
    /// The labels are not the ones the model was attached with.
    LabelsChanged,
    /// The artifact is missing and cannot be made again here.
    NotRebuildable,
    /// The model has no digest: a service, not a file.
    NotLocal,
}

impl Status {
    /// Whether the model can be used as attached.
    pub fn holds(self) -> bool {
        matches!(self, Self::Present | Self::Rebuilt | Self::NotLocal)
    }
}

/// A model that is a file: `<provider>/<model>@sha256:<hex>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Learned<'a> {
    /// `<provider>/<model>`, which trains it.
    pub trainer: &'a str,
    /// The digest of the artifact, without the `sha256:`.
    pub hex: &'a str,
}

/// What the store holds for the digest of a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Artifact {
    /// A file whose canonical form has the digest in its name.
    Present,
    /// A file under that name whose canonical form has another digest.
    Differs(String),
    Missing,
}

/// What to train again.
pub struct Refit<'a> {
    pub root: &'a Path,
    pub decision: &'a str,
    pub model: &'a Learned<'a>,
    /// The dataset the model learned from (`sha256:<hex>`), in the store.
    pub dataset: &'a str,
}

/// The parts of a model reference that name a file, when it is one.
pub fn learned(model: &str) -> Option<Learned<'_>> {
    let (trainer, digest) = model.split_once('@')?;
    let hex = digest.strip_prefix("sha256:")?;
    Some(Learned { trainer, hex })
}

/// What the model store of `root` holds for `hex`.
pub fn artifact(root: &Path, hex: &str) -> Result<Artifact> {
    let path = root
        .join(MODELS_DIR)
        .join("sha256")
        .join(format!("{hex}.json"));
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Artifact::Missing);
    };
    let document: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let actual = digest(&document);
    Ok(if actual == hex {
        Artifact::Present
    } else {
        Artifact::Differs(actual)
    })
}

/// Whether the dataset store of `root` holds `digest` (`sha256:<hex>`).
pub fn has_dataset(root: &Path, digest: &str) -> bool {
    dataset::path_of(root, digest).is_file()
}

/// Why `trainer` cannot make a model again, when it cannot: it must learn, and
/// must make the same model from the same dataset.
pub fn unrebuildable(trainer: &dyn Classifier) -> Option<String> {
    let manifest = trainer.manifest();
    let learns = manifest
        .capabilities
        .fine_tuning
        .is_some_and(|capability| capability.supported);
    if !learns {
        return Some("its trainer does not learn".to_owned());
    }
    (!manifest.deterministic).then(|| "its trainer is not deterministic".to_owned())
}

/// Trains the model again on its dataset and says whether it is the same
/// model, with what to tell about it.
pub fn refit(trainer: &dyn Classifier, request: &Refit) -> Result<(Status, Option<String>)> {
    let model = request.model;
    let done = trainer.train(&job(
        model.trainer,
        request.dataset,
        request.root,
        request.decision,
    ))?;
    if done.status != FineTuningJobStatus::Succeeded {
        let why = done
            .error
            .map_or_else(String::new, |e| format!(": {}", e.message));
        return Ok((
            Status::NotRebuildable,
            Some(format!("training failed{why}")),
        ));
    }
    let made = done
        .result_files
        .first()
        .and_then(|file| file.strip_prefix("sha256:"))
        .unwrap_or_default();
    if made == model.hex {
        return Ok((Status::Rebuilt, None));
    }
    Ok((
        Status::Differs,
        Some(format!(
            "training made sha256:{made}; the trainer or its weights changed since attach"
        )),
    ))
}
