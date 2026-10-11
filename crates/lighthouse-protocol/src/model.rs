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
    pub train: Call<TrainParams, TrainResult>,
}

/// What a model plugin offers: one entry of `models` in its `initialize` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelManifest {
    /// The model id the project binds in `[models]`; equals the plugin id.
    pub id: String,
    /// `classification`: a subject of a decision is a violation or not.
    pub task: String,
    /// Changes whenever the answers for the same input may change (the model
    /// name, the prompt template, the trained artifact).
    pub version: String,
    /// `classify`, and `train` when the model learns from labels.
    #[serde(default)]
    pub methods: Vec<String>,
}

/// An example of a decision, shown to the model as a few shots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Shot {
    /// `fail` for an invalid example, `pass` for a valid one.
    pub verdict: Verdict,
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

/// The verdict on a subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Fail,
    Pass,
    /// The code shown is not enough to tell.
    Abstain,
}

/// Params of `lighthouse/classify`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClassifyParams {
    pub decision: DecisionRef,
    pub subjects: Vec<Subject>,
}

/// The answer for one subject.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Answer {
    pub key: String,
    pub verdict: Verdict,
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
    pub label: Verdict,
    /// 0 to 1: how much the label is trusted.
    pub weight: f64,
    pub source: LabelSource,
}

/// Params of `lighthouse/train`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrainParams {
    pub decision: DecisionRef,
    pub subjects: Vec<TrainSubject>,
    /// A directory the host owns for this model and decision
    /// (`.lighthouse/models/<plugin>/<decision>/`); the artifact goes there.
    pub dir: String,
}

/// Result of `lighthouse/train`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrainResult {
    /// Numbers the trainer measured, such as out-of-fold precision and recall.
    #[serde(default)]
    pub metrics: BTreeMap<String, Value>,
    /// The artifact's hash; it becomes the model's version.
    pub version: String,
}
