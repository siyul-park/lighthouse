//! A model offered by a plugin process.

use std::sync::Arc;

use lighthouse_plugin::{
    Classifier, ClassifyParams, Error, FineTuningJob, FineTuningJobRequest, ModelManifest,
    SystemOneResponse,
};
use lighthouse_protocol::{self as wire, JobStatus};

use crate::client::Client;

/// A model that answers over the connection of its plugin.
pub(crate) struct Model {
    pub(crate) manifest: ModelManifest,
    pub(crate) client: Arc<Client>,
}

impl Model {
    fn failed(&self, reason: impl std::fmt::Display) -> Error {
        Error::Failed(format!("model `{}`: {reason}", self.manifest.id))
    }
}

impl Classifier for Model {
    fn manifest(&self) -> &ModelManifest {
        &self.manifest
    }

    /// The answer may only name questions that were asked.
    fn classify(&self, params: &ClassifyParams) -> Result<SystemOneResponse, Error> {
        let result: SystemOneResponse = self
            .client
            .call(wire::CLASSIFY, params)
            .map_err(|reason| self.failed(reason))?;
        if let Some(stray) = result
            .answers
            .keys()
            .find(|id| !params.request.questions.contains_key(*id))
        {
            return Err(self.failed(format!("it answers `{stray}`, which was not asked")));
        }
        Ok(result)
    }

    /// Asks for the fine-tuning job; the answer is final, `succeeded` or
    /// `failed`, and a failed job is an error here.
    fn train(&self, request: &FineTuningJobRequest) -> Result<FineTuningJob, Error> {
        if !self.manifest.trains {
            return Err(self.failed("it does not train"));
        }
        let job: FineTuningJob = self
            .client
            .call(wire::TRAIN, request)
            .map_err(|reason| self.failed(reason))?;
        match (&job.error, job.status) {
            (Some(error), _) => Err(self.failed(format!("{}: {}", error.code, error.message))),
            (None, JobStatus::Succeeded) if job.fine_tuned_model.is_some() => Ok(job),
            _ => Err(self.failed("the training did not succeed")),
        }
    }
}
