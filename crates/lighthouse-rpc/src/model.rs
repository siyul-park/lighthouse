//! A model offered by a plugin process.

use std::{collections::BTreeMap, fs, path::Path, sync::Arc};

use lighthouse_plugin::{
    Classifier, ClassifyParams, ClassifyResult, Error, ModelManifest, TrainParams, TrainResponse,
};
use lighthouse_protocol::{self as wire, JobStatus, Question, TrainLine};

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
    fn classify(&self, params: &ClassifyParams) -> Result<ClassifyResult, Error> {
        let result: ClassifyResult = self
            .client
            .call(wire::CLASSIFY, params)
            .map_err(|reason| self.failed(reason))?;
        if let Some(stray) = result
            .answers
            .keys()
            .find(|id| !params.questions.contains_key(*id))
        {
            return Err(self.failed(format!("it answers `{stray}`, which was not asked")));
        }
        Ok(result)
    }

    /// Writes the training file, asks for a fine-tuning job and reads the
    /// metrics from its first result file.
    fn train(&self, params: &TrainParams) -> Result<TrainResponse, Error> {
        if !self.manifest.trains {
            return Err(self.failed("it does not train"));
        }
        let dir = Path::new(&params.dir);
        let file = dir.join("training.jsonl");
        let question =
            BTreeMap::from([(params.decision.id.clone(), Question::of(&params.decision))]);
        let mut lines = String::new();
        for subject in &params.subjects {
            let line = TrainLine {
                state: subject.subject.state(),
                questions: question.clone(),
                label: subject.label,
                weight: subject.weight,
                source: subject.source,
            };
            lines.push_str(&serde_json::to_string(&line).map_err(|e| self.failed(e))?);
            lines.push('\n');
        }
        fs::write(&file, lines).map_err(|e| self.failed(format!("{}: {e}", file.display())))?;
        let seed = params
            .request_id
            .as_deref()
            .and_then(|id| u64::from_str_radix(&id[..id.len().min(15)], 16).ok())
            .unwrap_or_default();
        let request = wire::FineTuningJobRequest {
            model: self.manifest.id.clone(),
            training_file: file.to_string_lossy().into_owned(),
            validation_file: None,
            method: wire::Method {
                kind: "supervised".to_owned(),
                supervised: wire::Supervised::default(),
            },
            seed,
            suffix: None,
            metadata: BTreeMap::from([
                ("decision".to_owned(), params.decision.id.clone()),
                ("dir".to_owned(), params.dir.clone()),
            ]),
            options: params.options.clone(),
        };
        let job: wire::FineTuningJob = self
            .client
            .call(wire::TRAIN, request)
            .map_err(|reason| self.failed(reason))?;
        if let Some(error) = job.error {
            return Err(self.failed(format!("{}: {}", error.code, error.message)));
        }
        let (JobStatus::Succeeded, Some(artifact)) = (job.status, job.fine_tuned_model) else {
            return Err(self.failed("the training did not succeed"));
        };
        let metrics = job
            .result_files
            .first()
            .and_then(|file| fs::read_to_string(file).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Ok(TrainResponse { artifact, metrics })
    }
}
