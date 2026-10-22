//! A model offered by a plugin process.

use std::sync::Arc;

use lighthouse_plugin::{
    CreateFineTuningJobRequest, EditRequest, Edited, Error, FineTuningJob, Model as ModelInfo,
    ModelClient, SystemOneRequest, SystemOneResponse,
};
use lighthouse_protocol::{self as wire, FineTuningJobStatus};

use crate::{agent::Agent, client::Client};

/// A model that answers over the connection of its plugin.
pub(crate) struct Remote {
    pub(crate) manifest: ModelInfo,
    pub(crate) client: Arc<Client>,
    /// How its agent runs, when it is one: in a process of its own.
    pub(crate) agent: Agent,
}

impl ModelClient for Remote {
    fn manifest(&self) -> &ModelInfo {
        &self.manifest
    }

    /// The answer may only name questions that were asked.
    fn classify(&self, params: &SystemOneRequest) -> Result<SystemOneResponse, Error> {
        let result: SystemOneResponse = self
            .client
            .call(wire::CLASSIFY, params)
            .map_err(|reason| Error::model(&self.manifest.id, reason))?;
        if let Some(stray) = result
            .answers
            .keys()
            .find(|id| !params.questions.contains_key(*id))
        {
            return Err(Error::model(
                &self.manifest.id,
                format!("it answers `{stray}`, which was not asked"),
            ));
        }
        Ok(result)
    }

    /// Runs the agent of the plugin in the directory of the request, in a
    /// process started for it and sandboxed, never in the long-lived one.
    fn edit(&self, request: &EditRequest<'_>) -> Result<Edited, Error> {
        if !self.manifest.capabilities.edits() {
            return Err(Error::model(&self.manifest.id, "it does not edit"));
        }
        self.agent.edit(request)
    }

    /// Asks for the fine-tuning job; the answer is final, `succeeded` or
    /// `failed`, and a failed job is an error here.
    fn train(&self, request: &CreateFineTuningJobRequest) -> Result<FineTuningJob, Error> {
        if !self.manifest.capabilities.learns() {
            return Err(Error::model(&self.manifest.id, "it does not train"));
        }
        let job: FineTuningJob = self
            .client
            .call(wire::TRAIN, request)
            .map_err(|reason| Error::model(&self.manifest.id, reason))?;
        match (&job.error, job.status) {
            (Some(error), _) => Err(Error::model(
                &self.manifest.id,
                format!("{}: {}", error.code, error.message),
            )),
            (None, FineTuningJobStatus::Succeeded) if job.fine_tuned_model.is_some() => Ok(job),
            _ => Err(Error::model(
                &self.manifest.id,
                "the training did not succeed",
            )),
        }
    }
}
