//! The model methods: `lighthouse/classify` puts subjects of one decision to a
//! model, `lighthouse/train` fits a model on labelled subjects. Both carry the
//! subjects the same way, so that a model is trained on exactly what it is
//! later asked about.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Call, FineTuningJob, FineTuningJobRequest};

/// The model methods with their params and result. Exists to anchor the schema.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelMethods {
    #[serde(rename = "lighthouse/classify")]
    pub classify: Call<ClassifyParams, SystemOneResponse>,
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
pub struct Answer {
    /// `noul`.
    #[serde(rename = "type", default = "noul")]
    pub kind: String,
    /// The probability that the subject fails the decision, from 0 to 1.
    pub noul: f64,
}

/// The body of TypeSafe's System One API (`POST /v1/systemone`), for one
/// subject and the decisions judged on it, keyed by decision id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SystemOneRequest {
    pub state: State,
    /// The model asked: the part of the reference after the provider.
    pub model: String,
    pub questions: BTreeMap<String, Question>,
}

/// Params of `lighthouse/classify`: System One's body, and beside it the
/// `options` of the project's provider, for a plugin that needs them
/// (`spec`, `baseUrl`, `keyEnv`). The options are never part of the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClassifyParams {
    #[serde(flatten)]
    pub request: SystemOneRequest,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub options: serde_json::Map<String, Value>,
}

/// Result of `lighthouse/classify`: System One's response. Every question
/// asked is answered; a missing answer is an abstention.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SystemOneResponse {
    /// The model that answered, with its version when it has one.
    #[serde(default)]
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
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

fn noul() -> String {
    "noul".to_owned()
}
