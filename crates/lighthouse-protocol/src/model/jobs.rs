//! Training and the list of models, in the words of OpenAI's fine-tuning jobs
//! and models list.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{NOUL, Question, State};

/// One message of a supervised fine-tuning line, in OpenAI's chat format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessage {
    /// `system`, `user` or `assistant`.
    pub role: String,
    pub content: String,
}

/// One line of a training file, in OpenAI's supervised fine-tuning format with
/// the keys Lighthouse adds (mlops §2b). The `system` message is the
/// TypeSafe question as a JSON string, the `user` message the `state`, and
/// the `assistant` message the label as a TypeSafe answer, `{"type":"noul",
/// "noul":1}` for a fail and `0` for a pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrainingLine {
    pub messages: Vec<ChatMessage>,
    /// How much the label is trusted, 0 to 1.
    pub weight: f64,
    /// The cross-validation group: subjects of one group are never split
    /// between a training and a validation fold.
    pub group: String,
    pub decision: String,
    /// The subject's key.
    pub key: String,
    /// Who gave the label, as a PROV `wasAttributedTo` object.
    pub attribution: Value,
}

impl TrainingLine {
    /// The line for a subject asked `question` and labelled a fail or a pass.
    pub fn new(question: &Question, state: &State, fail: bool, keys: LineKeys<'_>) -> Self {
        let json = |value: Value| value.to_string();
        let noul = i32::from(fail);
        let message = |role: &str, content: String| ChatMessage {
            role: role.to_owned(),
            content,
        };
        Self {
            messages: vec![
                message(
                    "system",
                    json(serde_json::to_value(question).unwrap_or_default()),
                ),
                message(
                    "user",
                    json(serde_json::to_value(state).unwrap_or_default()),
                ),
                message(
                    "assistant",
                    json(serde_json::json!({"type": NOUL, "noul": noul})),
                ),
            ],
            weight: keys.weight,
            group: keys.group.to_owned(),
            decision: keys.decision.to_owned(),
            key: keys.key.to_owned(),
            attribution: keys.attribution,
        }
    }
}

/// What Lighthouse adds to a training line besides the chat messages.
pub struct LineKeys<'a> {
    pub weight: f64,
    pub group: &'a str,
    pub decision: &'a str,
    pub key: &'a str,
    pub attribution: Value,
}

/// Provider-specific supervised fine-tuning parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct FineTuneSupervisedHyperparameters {
    pub extra: serde_json::Map<String, Value>,
}

/// OpenAI's supervised fine-tuning method, with optional OpenAI parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FineTuneSupervisedMethod {
    #[serde(default)]
    pub hyperparameters: FineTuneSupervisedHyperparameters,
}

/// The method of a fine-tuning job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FineTuneMethod {
    /// `supervised`.
    #[serde(rename = "type")]
    pub kind: String,
    pub supervised: FineTuneSupervisedMethod,
}

/// The body of OpenAI's fine-tuning job creation (`POST /v1/fine_tuning/jobs`),
/// which `lighthouse/train` takes verbatim. `training_file` is the digest
/// (`sha256:<hex>`) of a JSON Lines file of [`TrainingLine`]s in the host's
/// dataset store; `metadata` names the `decision`, the `datasets` directory
/// the file is in (`<hex>.jsonl` under `sha256/`) and the `dir` the artifact
/// goes to.
// lighthouse-disable-next-line design/naming -- Matches OpenAI request type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateFineTuningJobRequest {
    pub model: String,
    pub training_file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_file: Option<String>,
    pub method: FineTuneMethod,
    pub seed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// The `options` of the project's provider. Not part of OpenAI's body.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub options: serde_json::Map<String, Value>,
}

/// How far a fine-tuning job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FineTuningJobStatus {
    ValidatingFiles,
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// Why a job failed, in OpenAI's words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FineTuneJobError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub param: Option<String>,
}

/// OpenAI's `FineTuningJob`. Over stdio `lighthouse/train` answers with the
/// final one: `fine_tuned_model` is the reference (`learned@sha256:<hex>`) and the
/// first of `result_files` is a JSON file of the metrics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FineTuningJob {
    pub id: String,
    pub created_at: u64,
    #[serde(default)]
    pub finished_at: Option<u64>,
    /// `fine_tuning.job`.
    pub object: String,
    pub model: String,
    pub status: FineTuningJobStatus,
    #[serde(default)]
    pub hyperparameters: Value,
    pub organization_id: String,
    #[serde(default)]
    pub trained_tokens: Option<u32>,
    pub training_file: String,
    #[serde(default)]
    pub validation_file: Option<String>,
    #[serde(default)]
    pub integrations: Option<Vec<Value>>,
    pub seed: u32,
    #[serde(default)]
    pub estimated_finish: Option<u32>,
    #[serde(default)]
    pub method: Option<FineTuneMethod>,
    #[serde(default)]
    pub metadata: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub fine_tuned_model: Option<String>,
    #[serde(default)]
    pub result_files: Vec<String>,
    #[serde(default)]
    pub error: Option<FineTuneJobError>,
}

/// What a model can do, as OpenAI's model object lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Supports {
    pub supported: bool,
}

/// The capabilities of a model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<Supports>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Supports>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fine_tuning: Option<Supports>,
}

impl Capabilities {
    /// A classifier, which learns from labels when `trains`.
    pub fn classifier(trains: bool) -> Self {
        Self {
            classification: Some(Supports { supported: true }),
            embedding: None,
            fine_tuning: trains.then_some(Supports { supported: true }),
        }
    }
}

/// One model: OpenAI's model object, with what Lighthouse adds
/// (`version`, `cost`, `deterministic`, `abstains`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Model {
    pub id: String,
    /// `model`.
    pub object: String,
    pub created: u32,
    pub owned_by: String,
    pub capabilities: Capabilities,
    pub version: String,
    pub cost: f64,
    pub deterministic: bool,
    pub abstains: bool,
}

impl Model {
    /// The model `id` of the plugin that owns it, at `version`: free, not
    /// deterministic and never abstaining until the caller says otherwise.
    pub fn new(id: &str, version: &str, capabilities: Capabilities) -> Self {
        Self {
            id: id.to_owned(),
            object: "model".to_owned(),
            created: 0,
            owned_by: id.to_owned(),
            capabilities,
            version: version.to_owned(),
            cost: 0.0,
            deterministic: false,
            abstains: false,
        }
    }
}

/// OpenAI's list object of models: how a plugin says which it offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ListModelResponse {
    /// `list`.
    pub object: String,
    pub data: Vec<Model>,
}

impl ListModelResponse {
    /// The list of `data`.
    pub fn new(data: Vec<Model>) -> Self {
        Self {
            object: "list".to_owned(),
            data,
        }
    }

    /// Whether the list has no model.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl Default for ListModelResponse {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}
