//! A model offered by a plugin process.

use std::{collections::BTreeSet, sync::Arc};

use lighthouse_plugin::{
    Classifier, ClassifyParams, ClassifyResult, Error, ModelManifest, TrainParams, TrainResult,
};
use lighthouse_protocol as wire;

use crate::client::Client;

/// A model that answers over the connection of its plugin.
pub(crate) struct Model {
    pub(crate) manifest: ModelManifest,
    pub(crate) client: Arc<Client>,
}

impl Model {
    fn offers(&self, method: &str) -> bool {
        self.manifest.methods.iter().any(|m| m == method)
    }

    fn failed(&self, reason: impl std::fmt::Display) -> Error {
        Error::Failed(format!("model `{}`: {reason}", self.manifest.id))
    }
}

impl Classifier for Model {
    fn manifest(&self) -> &ModelManifest {
        &self.manifest
    }

    /// The answer must name every subject once and no other.
    fn classify(&self, params: &ClassifyParams) -> Result<ClassifyResult, Error> {
        let result: ClassifyResult = self
            .client
            .call(wire::CLASSIFY, params)
            .map_err(|reason| self.failed(reason))?;
        let asked: BTreeSet<&str> = params.subjects.iter().map(|s| s.key.as_str()).collect();
        let answered: BTreeSet<&str> = result.results.iter().map(|a| a.key.as_str()).collect();
        if answered != asked || result.results.len() != asked.len() {
            return Err(self.failed("its answers do not name exactly the subjects asked"));
        }
        Ok(result)
    }

    fn train(&self, params: &TrainParams) -> Result<TrainResult, Error> {
        if !self.offers("train") {
            return Err(self.failed("it does not train"));
        }
        self.client
            .call(wire::TRAIN, params)
            .map_err(|reason| self.failed(reason))
    }
}
