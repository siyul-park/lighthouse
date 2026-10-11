//! The model methods: `lighthouse/classify` puts subjects of one decision to a
//! model, `lighthouse/train` fits a model on labelled subjects. Both carry the
//! subjects the same way, so that a model is trained on exactly what it is
//! later asked about.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Call;

/// The model methods with their params and result. Exists to anchor the schema.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelMethods {
    #[serde(rename = "lighthouse/classify")]
    pub classify: Call<ClassifyParams, ClassifyResult>,
    #[serde(rename = "lighthouse/train")]
    pub train: Call<FineTuningJobRequest, FineTuningJob>,
}

/// What a model plugin offers: one entry of `models` in its `initialize` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelManifest {
    /// The `Model` resource's name; for a plugin's model, the plugin's id.
    #[serde(rename = "name")]
    pub id: String,
    /// The tasks it can do: `classification` (a subject of a decision is a
    /// violation or not); later ones may join.
    pub tasks: Vec<String>,
    /// Changes whenever the answers for the same input may change (the model
    /// name, the prompt template, the trained artifact).
    pub version: String,
    /// The model learns from labels: it answers `lighthouse/train`.
    #[serde(default)]
    pub trains: bool,
    /// The same input always gets the same answer.
    pub deterministic: bool,
    /// The model may answer `abstain`, and the subject then goes on to the
    /// next binding.
    pub abstains: bool,
    /// What one answer costs, in any unit the project budgets; 0 for a model
    /// that runs locally for free. The run's budget and its report scope
    /// limit only the models with a cost above 0.
    pub cost: f64,
}

/// An example of a decision, shown to the model as a few shots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Shot {
    /// `fail` for an invalid example, `pass` for a valid one.
    pub kind: ResultKind,
    pub language: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// The decision the subjects are judged against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DecisionRef {
    pub id: String,
    pub requirement: String,
    /// What the model is told; the requirement unless the check says more.
    pub prompt: String,
    #[serde(default)]
    pub examples: Vec<Shot>,
}

/// A symbol near the subject: a caller or callee.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Neighbor {
    pub id: String,
    pub kind: String,
    /// `caller` or `callee`.
    pub relation: String,
    /// The neighbor's source when it is in the subject's file, truncated.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

/// What the host knows about a subject besides its text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubjectContext {
    /// The select's facts for the subject, as the builder made them.
    #[serde(default)]
    pub facts: Value,
    #[serde(default)]
    pub neighbors: Vec<Neighbor>,
}

/// One thing to judge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Subject {
    /// Identifies the subject within the request; answers repeat it.
    pub key: String,
    /// `symbol`, `function`, `file`, `test`, `comment` or `event`.
    pub kind: String,
    pub id: String,
    pub language: String,
    /// Project-relative, `/`-separated.
    pub file: String,
    /// The subject's source by extent.
    pub text: String,
    #[serde(default)]
    pub context: SubjectContext,
}

/// How a subject came out, in the words of SARIF result kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ResultKind {
    Fail,
    Pass,
    /// The code shown is not enough to tell: someone else must look.
    Review,
}

/// The part of a subject a model reads: TypeSafe System One's `state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct State {
    pub language: String,
    /// Project-relative, `/`-separated.
    pub file: String,
    /// The subject's source by extent.
    pub text: String,
    pub facts: Value,
    pub neighbors: Vec<Neighbor>,
}

/// An example under a criterion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Example {
    pub language: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// What a criterion means, and what it looks like.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Criterion {
    pub meaning: String,
    pub examples: Vec<Example>,
}

/// The two sides of a yes/no question: `true` is that the subject fails the
/// decision, `false` that it passes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Criteria {
    #[serde(rename = "true")]
    pub fail: Criterion,
    #[serde(rename = "false")]
    pub pass: Criterion,
}

/// What a question is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Instructions {
    pub requirement: String,
    /// What the model is told; the requirement unless the check says more.
    pub prompt: String,
}

/// One yes/no question: System One's `noul`, the probability of yes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Question {
    /// `noul`.
    #[serde(rename = "type")]
    pub kind: String,
    pub instructions: Instructions,
    pub criteria: Criteria,
}

/// The answer to a question: P(fail).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionAnswer {
    /// `noul`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The probability that the subject fails the decision, from 0 to 1.
    pub noul: f64,
}

/// Params of `lighthouse/classify`: the body of TypeSafe's System One API
/// (`POST /v1/systemone`), for one subject and the decisions judged on it,
/// keyed by decision id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClassifyParams {
    pub state: State,
    /// The model asked: the part of the reference after the provider.
    pub model: String,
    pub questions: BTreeMap<String, Question>,
    /// The `options` of the project's provider, for a plugin that needs them
    /// (`spec`, `baseUrl`, `keyEnv`). Not part of System One's body.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub options: serde_json::Map<String, Value>,
}

/// Result of `lighthouse/classify`: System One's response. Every question
/// asked is answered; a missing answer is an abstention.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClassifyResult {
    /// The model that answered, with its version when it has one.
    pub model: String,
    pub answers: BTreeMap<String, QuestionAnswer>,
    #[serde(default)]
    pub usage: Value,
}

impl Question {
    /// The question a decision asks: it fails when `true`, its invalid
    /// examples show what that looks like, its valid examples the other side.
    pub fn of(decision: &DecisionRef) -> Self {
        let side = |kind: ResultKind, meaning: String| Criterion {
            meaning,
            examples: decision
                .examples
                .iter()
                .filter(|shot| shot.kind == kind)
                .map(|shot| Example {
                    language: shot.language.clone(),
                    code: shot.code.clone(),
                    note: shot.note.clone(),
                })
                .collect(),
        };
        Self {
            kind: "noul".to_owned(),
            instructions: Instructions {
                requirement: decision.requirement.clone(),
                prompt: decision.prompt.clone(),
            },
            criteria: Criteria {
                fail: side(
                    ResultKind::Fail,
                    format!(
                        "The subject violates the requirement: {}",
                        decision.requirement
                    ),
                ),
                pass: side(
                    ResultKind::Pass,
                    format!(
                        "The subject complies with the requirement: {}",
                        decision.requirement
                    ),
                ),
            },
        }
    }
}

impl Subject {
    /// What a model reads of the subject.
    pub fn state(&self) -> State {
        State {
            language: self.language.clone(),
            file: self.file.clone(),
            text: self.text.clone(),
            facts: self.context.facts.clone(),
            neighbors: self.context.neighbors.clone(),
        }
    }
}

/// Where a training label came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LabelSource {
    Example,
    Baseline,
    Person,
    Model,
}

/// A labelled subject.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrainSubject {
    #[serde(flatten)]
    pub subject: Subject,
    /// `fail` or `pass`.
    pub label: ResultKind,
    /// 0 to 1: how much the label is trusted.
    pub weight: f64,
    pub source: LabelSource,
}

/// Params of `lighthouse/train`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrainParams {
    /// Names the request, so that a retry starts no second job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub decision: DecisionRef,
    pub subjects: Vec<TrainSubject>,
    /// The `options` of the project's model instance.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub options: serde_json::Map<String, Value>,
    /// A directory the host owns for this model and decision
    /// (`.lighthouse/models/<plugin>/<decision>/`); the artifact goes there.
    pub dir: String,
}

/// What a finished training gives the host: the artifact, content-addressed
/// (`sha256:<hex>`), and the numbers the trainer measured, such as
/// out-of-fold precision and recall.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrainResponse {
    pub artifact: String,
    #[serde(default)]
    pub metrics: BTreeMap<String, Value>,
}

/// One line of a training file: what a model is asked, and the answer the
/// labels give.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrainLine {
    pub state: State,
    pub questions: BTreeMap<String, Question>,
    /// `fail` or `pass`.
    pub label: ResultKind,
    pub weight: f64,
    pub source: LabelSource,
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
/// which `lighthouse/train` takes verbatim. `training_file` is the path of a
/// JSON Lines file of [`TrainLine`]s; `metadata` names the `decision` and the
/// `dir` the artifact goes to.
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

/// The training file: one [`TrainLine`] per labelled subject, as JSON Lines.
pub fn training_file(decision: &DecisionRef, subjects: &[TrainSubject]) -> String {
    let question = BTreeMap::from([(decision.id.clone(), Question::of(decision))]);
    let mut text = String::new();
    for subject in subjects {
        let line = TrainLine {
            state: subject.subject.state(),
            questions: question.clone(),
            label: subject.label,
            weight: subject.weight,
            source: subject.source,
        };
        text.push_str(&serde_json::to_string(&line).unwrap_or_default());
        text.push('\n');
    }
    text
}
