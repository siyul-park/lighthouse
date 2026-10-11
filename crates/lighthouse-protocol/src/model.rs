//! The model methods: `lighthouse/classify` puts subjects of one decision to a
//! model, `lighthouse/train` fits a model on labelled subjects. Both carry the
//! subjects the same way, so that a model is trained on exactly what it is
//! later asked about.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Call, Span};

/// The model methods with their params and result. Exists to anchor the schema.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelMethods {
    #[serde(rename = "lighthouse/classify")]
    pub classify: Call<ClassifyParams, ClassifyResult>,
    #[serde(rename = "lighthouse/train")]
    pub train: Call<TrainParams, Operation>,
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

/// Params of `lighthouse/classify`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClassifyParams {
    pub decision: DecisionRef,
    pub subjects: Vec<Subject>,
    /// The `options` of the project's model instance: which of the plugin's
    /// models is asked, and how.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub options: serde_json::Map<String, Value>,
}

/// The answer for one subject.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Answer {
    pub key: String,
    pub kind: ResultKind,
    /// 0 to 1.
    pub confidence: f64,
    /// One sentence.
    #[serde(default)]
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
}

/// Which model answered, and which version of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelRef {
    pub id: String,
    pub version: String,
}

/// Result of `lighthouse/classify`: every requested key is answered once; an
/// unknown or missing key makes the batch an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClassifyResult {
    pub results: Vec<Answer>,
    pub model: ModelRef,
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

/// What a finished training gives: the artifact, content-addressed
/// (`sha256:<hex>`), and the numbers the trainer measured, such as
/// out-of-fold precision and recall.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrainResponse {
    pub artifact: String,
    #[serde(default)]
    pub metrics: BTreeMap<String, Value>,
}

/// A long-running operation (AIP-151). Over stdio `lighthouse/train` answers
/// with one that is `done`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Operation {
    /// `operations/{id}`.
    pub name: String,
    pub done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    /// Set when the operation failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Status>,
    /// Set when it succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<TrainResponse>,
}

/// An error in the words of AIP-193: a canonical status and a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Status {
    /// `INVALID_ARGUMENT`, `UNAVAILABLE`, `RESOURCE_EXHAUSTED`, ...
    pub status: String,
    pub message: String,
}
