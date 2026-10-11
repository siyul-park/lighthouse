//! Training and the list of models, in the words of OpenAI's fine-tuning jobs
//! and models list.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ModelManifest, Question, ResultKind, State};

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
    /// The line for a subject asked `question` and labelled `label` (`Review`
    /// is no label and is read as a pass).
    pub fn new(question: &Question, state: &State, label: ResultKind, keys: LineKeys<'_>) -> Self {
        let json = |value: Value| value.to_string();
        let noul = i32::from(label == ResultKind::Fail);
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
                    json(serde_json::json!({"type": "noul", "noul": noul})),
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

/// The supervised method of a fine-tuning job.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Supervised {
    #[serde(default)]
    pub hyperparameters: serde_json::Map<String, Value>,
}

/// The method of a fine-tuning job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Method {
    /// `supervised`.
    #[serde(rename = "type")]
    pub kind: String,
    pub supervised: Supervised,
}

/// The body of OpenAI's fine-tuning job creation (`POST /v1/fine_tuning/jobs`),
/// which `lighthouse/train` takes verbatim. `training_file` is the digest
/// (`sha256:<hex>`) of a JSON Lines file of [`TrainingLine`]s in the host's
/// dataset store; `metadata` names the `decision`, the `datasets` directory
/// the file is in (`<hex>.jsonl` under `sha256/`) and the `dir` the artifact
/// goes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FineTuningJobRequest {
    pub model: String,
    pub training_file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation_file: Option<String>,
    pub method: Method,
    pub seed: u64,
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
pub enum JobStatus {
    ValidatingFiles,
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// Why a job failed, in OpenAI's words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub param: Option<String>,
}

/// OpenAI's `FineTuningJob`. Over stdio `lighthouse/train` answers with the
/// final one: `fine_tuned_model` is the artifact (`sha256:<hex>`) and the
/// first of `result_files` is a JSON file of the metrics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FineTuningJob {
    pub id: String,
    /// `fine_tuning.job`.
    pub object: String,
    pub model: String,
    pub status: JobStatus,
    #[serde(default)]
    pub fine_tuned_model: Option<String>,
    #[serde(default)]
    pub result_files: Vec<String>,
    #[serde(default)]
    pub error: Option<JobError>,
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

/// One model of the list: OpenAI's model object, with what Lighthouse adds
/// (`version`, `cost`, `deterministic`, `abstains`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelEntry {
    pub id: String,
    /// `model`.
    pub object: String,
    pub created: u64,
    pub owned_by: String,
    pub capabilities: Capabilities,
    pub version: String,
    pub cost: f64,
    pub deterministic: bool,
    pub abstains: bool,
}

/// OpenAI's list object of models: how a plugin says which it offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelList {
    /// `list`.
    pub object: String,
    pub data: Vec<ModelEntry>,
}

impl Default for ModelList {
    fn default() -> Self {
        Self {
            object: "list".to_owned(),
            data: Vec::new(),
        }
    }
}

impl ModelList {
    /// The list of these models.
    pub fn of(models: &[ModelManifest]) -> Self {
        let supports = |on: bool| on.then_some(Supports { supported: true });
        Self {
            object: "list".to_owned(),
            data: models
                .iter()
                .map(|m| ModelEntry {
                    id: m.id.clone(),
                    object: "model".to_owned(),
                    created: 0,
                    owned_by: m.id.clone(),
                    capabilities: Capabilities {
                        classification: supports(m.tasks.iter().any(|t| t == "classification")),
                        embedding: supports(m.tasks.iter().any(|t| t == "embedding")),
                        fine_tuning: supports(m.trains),
                    },
                    version: m.version.clone(),
                    cost: m.cost,
                    deterministic: m.deterministic,
                    abstains: m.abstains,
                })
                .collect(),
        }
    }

    /// Whether the list has no model.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The models as the host handles them.
    pub fn manifests(&self) -> Vec<ModelManifest> {
        self.data
            .iter()
            .map(|e| ModelManifest {
                id: e.id.clone(),
                tasks: [
                    ("classification", e.capabilities.classification),
                    ("embedding", e.capabilities.embedding),
                ]
                .into_iter()
                .filter(|(_, on)| on.is_some_and(|s| s.supported))
                .map(|(task, _)| task.to_owned())
                .collect(),
                version: e.version.clone(),
                trains: e.capabilities.fine_tuning.is_some_and(|s| s.supported),
                deterministic: e.deterministic,
                abstains: e.abstains,
                cost: e.cost,
            })
            .collect()
    }
}
