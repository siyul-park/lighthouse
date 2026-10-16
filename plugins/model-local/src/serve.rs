//! The plugin as the protocol sees it: the handshake, `classify` with a
//! trained model and `train`. The one model it offers is `learned`; what it
//! answers with is the artifact its reference names, `learned@sha256:<hex>`.

use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_protocol::{
    Answer, Capabilities, CreateFineTuningJobRequest, FineTuneJobError, FineTuningJob,
    FineTuningJobStatus, Handler, HandlerError, IndexParams, IndexResult, InitializeParams,
    InitializeResult, ListModelResponse, MODELS_DIR, Model, Supports, SystemOneRequest,
    SystemOneResponse, VERSION,
};
use serde_json::{Value, json};

use crate::{
    artifact::{self, Loaded},
    band::rounded,
    candidates::similarities,
    embed::{Embedders, Open},
    rows::Line,
    train::{self, Trained},
};

/// The plugin's id.
pub const ID: &str = "model-local";
/// The model part of a reference it serves: `learned@sha256:<hex>`.
pub const MODEL: &str = "learned";

pub struct Plugin {
    embedders: Embedders,
    /// The store of models, under the project root.
    dir: PathBuf,
    loaded: BTreeMap<String, Loaded>,
}

impl Plugin {
    pub fn new(open: Open) -> Self {
        Self {
            embedders: Embedders::new(open),
            dir: PathBuf::from(MODELS_DIR),
            loaded: BTreeMap::new(),
        }
    }
}

impl Handler for Plugin {
    fn initialize(&mut self, params: InitializeParams) -> Result<InitializeResult, HandlerError> {
        if params.protocol_version != VERSION {
            return Err(format!("speaks protocol {VERSION}").into());
        }
        self.dir = PathBuf::from(params.root).join(MODELS_DIR);
        Ok(InitializeResult {
            id: ID.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: VERSION.to_owned(),
            languages: Vec::new(),
            models: ListModelResponse {
                object: "list".to_owned(),
                data: vec![Model {
                    id: ID.to_owned(),
                    object: "model".to_owned(),
                    created: 0,
                    owned_by: ID.to_owned(),
                    capabilities: Capabilities {
                        classification: Some(Supports { supported: true }),
                        embedding: None,
                        fine_tuning: Some(Supports { supported: true }),
                    },
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                    deterministic: true,
                    abstains: false,
                    cost: 0.0,
                }],
            },
        })
    }

    fn index(&mut self, _: IndexParams) -> Result<IndexResult, HandlerError> {
        Err("model-local indexes no language".into())
    }

    fn classify(&mut self, params: SystemOneRequest) -> Result<SystemOneResponse, HandlerError> {
        let hex = digest_of(&params.model)?;
        if !self.loaded.contains_key(hex) {
            self.loaded
                .insert(hex.to_owned(), artifact::load(&self.dir, hex)?);
        }
        Ok(answer(&self.loaded[hex], &mut self.embedders, params)?)
    }

    fn train(
        &mut self,
        request: CreateFineTuningJobRequest,
    ) -> Result<FineTuningJob, HandlerError> {
        Ok(match train::run(&request, &mut self.embedders) {
            Ok(trained) => succeeded(request, &trained),
            Err(message) => failed(request, message),
        })
    }
}

/// The digest (hex) in a reference `learned@sha256:<hex>`.
fn digest_of(model: &str) -> Result<&str, String> {
    match model.strip_prefix(MODEL) {
        Some("") => Err(format!(
            "`{MODEL}` is not trained: train first with `lighthouse model train <decision> --model local/{MODEL}`, \
             then use the reference it prints (`{MODEL}@sha256:<hex>`)"
        )),
        Some(rest) => rest
            .strip_prefix("@sha256:")
            .ok_or_else(|| format!("`{model}` is not `{MODEL}@sha256:<hex>`")),
        None => Err(format!("model-local offers `{MODEL}`, not `{model}`")),
    }
}

/// What the model says to the one decision it was trained for.
fn answer(
    loaded: &Loaded,
    embedders: &mut Embedders,
    params: SystemOneRequest,
) -> Result<SystemOneResponse, String> {
    if let Some(other) = params.questions.keys().find(|key| **key != loaded.decision) {
        return Err(format!(
            "{} was trained for `{}`, not `{other}`",
            params.model, loaded.decision
        ));
    }
    let noul = rounded(
        loaded
            .model
            .predict(&row_of(loaded, embedders, &params.state.text)?),
    );
    let answers = params
        .questions
        .into_keys()
        .map(|id| {
            let kind = "noul".to_owned();
            (id, Answer { kind, noul })
        })
        .collect();
    Ok(SystemOneResponse {
        model: params.model,
        answers,
        usage: Value::Null,
    })
}

/// The subject as the model reads it. The rate reads nothing.
fn row_of(loaded: &Loaded, embedders: &mut Embedders, text: &str) -> Result<Line, String> {
    let (vector, sims) = if loaded.model.embeds() {
        let name = loaded.embedder.split('@').next().unwrap_or_default();
        let embedder = embedders.get(name)?;
        if embedder.id() != loaded.embedder {
            return Err(format!(
                "the model was trained with {}, not {}",
                loaded.embedder,
                embedder.id()
            ));
        }
        let mut vectors = embedder.vectors(&[text.to_owned()])?;
        let vector: Vec<f64> = vectors
            .pop()
            .unwrap_or_default()
            .into_iter()
            .map(f64::from)
            .collect();
        let sims = similarities(&vector, &loaded.invalid, &loaded.valid);
        (vector, sims)
    } else {
        (Vec::new(), [0.0; 4])
    };
    Ok(Line {
        text: text.to_owned(),
        vector,
        sims,
        weight: 1.0,
        ..Line::default()
    })
}

fn succeeded(request: CreateFineTuningJobRequest, trained: &Trained) -> FineTuningJob {
    let hyperparameters = json!({"chosen": trained.fitted.chosen, "embedder": trained.embedder});
    let reference = format!("{MODEL}@sha256:{}", trained.hex);
    let result = format!("sha256:{}", trained.hex);
    job(request, FineTuningJobStatus::Succeeded, |job| {
        job.hyperparameters = hyperparameters;
        job.fine_tuned_model = Some(reference);
        job.result_files = vec![result];
    })
}

fn failed(request: CreateFineTuningJobRequest, message: String) -> FineTuningJob {
    job(request, FineTuningJobStatus::Failed, |job| {
        job.error = Some(FineTuneJobError {
            code: "train_failed".to_owned(),
            message,
            param: None,
        });
    })
}

fn job(
    request: CreateFineTuningJobRequest,
    status: FineTuningJobStatus,
    fill: impl FnOnce(&mut FineTuningJob),
) -> FineTuningJob {
    let mut job = FineTuningJob {
        id: format!(
            "ftjob-{}",
            request.training_file.trim_start_matches("sha256:")
        ),
        created_at: 0,
        finished_at: None,
        object: "fine_tuning.job".to_owned(),
        model: request.model,
        status,
        hyperparameters: Value::Null,
        organization_id: ID.to_owned(),
        trained_tokens: None,
        training_file: request.training_file,
        validation_file: request.validation_file,
        integrations: None,
        seed: request.seed,
        estimated_finish: None,
        method: Some(request.method),
        metadata: Some(request.metadata),
        fine_tuned_model: None,
        result_files: Vec::new(),
        error: None,
    };
    fill(&mut job);
    job
}
