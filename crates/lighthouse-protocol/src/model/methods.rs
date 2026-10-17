//! The model methods: `lighthouse/classify` puts subjects of one decision to a
//! model, `lighthouse/train` fits a model on labelled subjects. Both carry the
//! subjects the same way, so that a model is trained on exactly what it is
//! later asked about.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Call, ChatCall, CreateFineTuningJobRequest, FineTuningJob};

/// The `type` of a question and of its answer: System One's `noul`, the
/// probability of yes.
pub const NOUL: &str = "noul";

/// The model methods with their params and result. Exists to anchor the schema.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelMethods {
    #[serde(rename = "lighthouse/classify")]
    pub classify: Call<SystemOneRequest, SystemOneResponse>,
    #[serde(rename = "lighthouse/chat")]
    pub chat: ChatCall,
    #[serde(rename = "lighthouse/train")]
    pub train: Call<CreateFineTuningJobRequest, FineTuningJob>,
}

/// One valid or invalid example of a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Example {
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
    pub valid: Vec<Example>,
    #[serde(default)]
    pub invalid: Vec<Example>,
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

/// One thing to judge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Subject {
    /// Identifies the subject within the request; answers repeat it.
    pub key: String,
    /// `symbol`, `function`, `file`, `test`, `comment` or `event`.
    pub kind: String,
    pub id: String,
    /// What a model reads of it.
    pub state: State,
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
    /// [`NOUL`].
    #[serde(rename = "type")]
    pub kind: String,
    pub instructions: Instructions,
    pub criteria: Criteria,
}

/// The answer to a question: P(fail).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Answer {
    /// [`NOUL`].
    #[serde(rename = "type", default = "noul_kind")]
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
    /// Provider options, omitted when empty and stripped by HTTP providers.
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
    pub fn new(decision: &DecisionRef) -> Self {
        Self {
            kind: NOUL.to_owned(),
            instructions: Instructions {
                requirement: decision.requirement.clone(),
                prompt: decision.prompt.clone(),
            },
            criteria: Criteria {
                fail: Criterion {
                    meaning: format!(
                        "The subject violates the requirement: {}",
                        decision.requirement
                    ),
                    examples: decision.invalid.clone(),
                },
                pass: Criterion {
                    meaning: format!(
                        "The subject complies with the requirement: {}",
                        decision.requirement
                    ),
                    examples: decision.valid.clone(),
                },
            },
        }
    }
}

impl Answer {
    /// The answer that the subject fails with probability `noul`.
    pub fn new(noul: f64) -> Self {
        Self {
            kind: NOUL.to_owned(),
            noul,
        }
    }
}

fn noul_kind() -> String {
    NOUL.to_owned()
}
