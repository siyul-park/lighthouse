//! `lighthouse model rebuild`, the host's half: whether the artifact of an
//! attached model is on this machine, and training it again from the dataset
//! it learned from when it is not. A model that learns from labels is a file
//! named by its digest and made by a deterministic trainer, so the same
//! dataset makes the same file.

use std::path::Path;

use lighthouse_model::Result;
use lighthouse_plugin::{FineTuningJobStatus, ModelClient};
use lighthouse_protocol::hash::digest;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::{artifact, dataset, train::fine_tune};

/// How a rebuild went for one attached model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Learned<'a> {
    /// The whole reference.
    pub model: &'a str,
    /// `<provider>/<model>`, which trains it.
    pub trainer: &'a str,
    /// The digest of the artifact, without the `sha256:`.
    pub hex: &'a str,
}

impl<'a> Learned<'a> {
    /// The parts of a model reference that name a file, when it is one.
    pub fn parse(model: &'a str) -> Option<Self> {
        let (trainer, digest) = model.split_once('@')?;
        let hex = digest.strip_prefix("sha256:")?;
        Some(Self {
            model,
            trainer,
            hex,
        })
    }

    /// The dataset (`sha256:<hex>`) the card of the artifact names as the file
    /// it was trained on; none for a model without an artifact here.
    pub(crate) fn dataset(&self, root: &Path) -> Result<Option<String>> {
        let document = artifact::read(root, self.hex)?;
        Ok(document
            .as_ref()
            .and_then(|document| document.pointer("/card/trainingFile"))
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    /// What the model store of `root` holds for the model: `Present` when the
    /// file's canonical form has the digest in its name, `Differs` when it has
    /// another, and `None` when the file is not there.
    pub fn check(&self, root: &Path) -> Result<Option<(Status, Option<String>)>> {
        let Some(document) = artifact::read(root, self.hex)? else {
            return Ok(None);
        };
        let actual = digest(&document);
        Ok(Some(if actual == self.hex {
            (Status::Present, None)
        } else {
            let note = format!("the file holds sha256:{actual}, not sha256:{}", self.hex);
            (Status::Differs, Some(note))
        }))
    }

    /// Trains the model again on the dataset it was attached with: the one in
    /// the store, else the one `today` makes when it makes the same. Says
    /// whether it is the same model, with what to tell about it.
    pub fn rebuild(
        &self,
        trainer: &dyn ModelClient,
        root: &Path,
        decision: &str,
        attached: &str,
        today: impl FnOnce() -> Result<String>,
    ) -> Result<(Status, Option<String>)> {
        if let Some(why) = unrebuildable(trainer) {
            return Ok((Status::NotRebuildable, Some(why)));
        }
        if !dataset::path_of(root, attached).is_file() {
            let made = today()?;
            if made != attached {
                let note = format!(
                    "labels changed since attach (dataset {made}, attached from {attached}); train and attach again"
                );
                return Ok((Status::LabelsChanged, Some(note)));
            }
        }
        let done = fine_tune(trainer, self.trainer, attached, root, decision)?;
        if done.status != FineTuningJobStatus::Succeeded {
            let why = done
                .error
                .map_or_else(String::new, |e| format!(": {}", e.message));
            return Ok((
                Status::NotRebuildable,
                Some(format!("training failed{why}")),
            ));
        }
        let made = artifact::made(&done).unwrap_or_default();
        if made == self.hex {
            return Ok((Status::Rebuilt, None));
        }
        Ok((
            Status::Differs,
            Some(format!(
                "training made sha256:{made}; the trainer or its weights changed since attach"
            )),
        ))
    }
}

/// Why `trainer` cannot make a model again, when it cannot: it must learn, and
/// must make the same model from the same dataset.
pub fn unrebuildable(trainer: &dyn ModelClient) -> Option<String> {
    let manifest = trainer.manifest();
    if !manifest.capabilities.learns() {
        return Some("its trainer does not learn".to_owned());
    }
    (!manifest.deterministic).then(|| "its trainer is not deterministic".to_owned())
}
