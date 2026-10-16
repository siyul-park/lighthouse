//! Models as the rules see them: a [`Classifier`] is a plugin's model, and
//! [`Models`] is what a run offers a rule to put subjects to the models its
//! decision is bound to. The wire types are the protocol's: a model is trained
//! on what it is later asked about.

pub use lighthouse_protocol::MODELS_DIR;
pub use lighthouse_protocol::{
    Answer, Capabilities, CreateFineTuningJobRequest, DecisionRef, Example, FineTuneJobError,
    FineTuneMethod, FineTuneSupervisedHyperparameters, FineTuneSupervisedMethod, FineTuningJob,
    FineTuningJobStatus, LineKeys, ListModelResponse, Model, Neighbor, Question, ResultKind, State,
    Subject, SubjectContext, Supports, SystemOneRequest, SystemOneResponse, TrainingLine,
};

use lighthouse_model::Diagnostic;

use crate::Error;

/// The probability at which an uncalibrated model's answer is a `fail`; a
/// model that has not been calibrated never clears a subject.
pub const UPPER: f64 = 0.9;

/// A model offered by a plugin.
pub trait Classifier: Send + Sync {
    fn manifest(&self) -> &Model;
    /// Answers every subject of the request, or fails as a whole.
    fn classify(&self, params: &SystemOneRequest) -> Result<SystemOneResponse, Error>;
    /// Runs a fine-tuning job; only a model whose manifest supports `fine_tuning` does.
    /// The job is final: `succeeded` or `failed`.
    fn train(&self, request: &CreateFineTuningJobRequest) -> Result<FineTuningJob, Error>;
}

/// Subjects of one decision to be judged.
#[derive(Debug, Clone)]
pub struct Ask {
    /// The rule of the decision.
    pub rule: String,
    /// Changes whenever the way the rule judges changes; cached answers of
    /// another revision are not used.
    pub revision: String,
    /// P(fail) at or above which a model's answer is a `fail`; below it the
    /// subject is left for review.
    pub upper: f64,
    pub decision: DecisionRef,
    pub subjects: Vec<Subject>,
}

/// A subject a model failed.
#[derive(Debug, Clone, PartialEq)]
pub struct Decided {
    /// max(p, 1 - p) of the probability the model gave.
    pub confidence: f64,
    /// The model that decided, `<provider>/<model>@<version>`.
    pub model: String,
}

/// Why a subject was left undecided: it is a candidate for agent review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Open {
    /// Every model of the cascade abstained or was unsure.
    Undecided,
    /// No model is bound to the decision, or models are off.
    Unbound,
    /// The run's budget was spent.
    OverBudget,
    /// A model failed; the reason.
    Failed(String),
}

impl std::fmt::Display for Open {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undecided => f.write_str("no model decided"),
            Self::Unbound => f.write_str("no model is bound"),
            Self::OverBudget => f.write_str("the model budget was spent"),
            Self::Failed(reason) => write!(f, "a model failed: {reason}"),
        }
    }
}

/// The outcome for one subject.
#[derive(Debug, Clone, PartialEq)]
pub enum Judged {
    Decided(Decided),
    Open(Open),
}

/// How a queued ask turns into diagnostics once its subjects are judged.
pub type Settle = Box<dyn FnOnce(Vec<Judged>) -> Vec<Diagnostic> + Send + Sync>;

/// What a run offers rules that ask models.
pub trait Models: Send + Sync {
    /// Whether the run keeps the subjects rules put to it, for training.
    fn collecting(&self) -> bool {
        false
    }

    /// Whether the run puts the subjects `rule` suspects to models: a model is
    /// bound to its decision, or the run asks for model work. Otherwise the
    /// rule skips its `review` route.
    fn asks(&self, _rule: &str) -> bool {
        false
    }

    /// Keeps subjects seen.
    fn observe(&self, _subjects: Vec<Subject>) {}

    /// Queues `ask` under `group` (the file a rule is checking): the models are
    /// asked when the group is flushed, so that every decision that needs a
    /// model on a subject is put in one request. `settle` then turns the
    /// outcome per subject, in order, into the rule's diagnostics. A run that
    /// cannot queue settles at once and hands the diagnostics back.
    fn queue(&self, _group: &str, ask: Ask, settle: Settle) -> Option<Vec<Diagnostic>> {
        let unbound = ask
            .subjects
            .iter()
            .map(|_| Judged::Open(Open::Unbound))
            .collect();
        Some(settle(unbound))
    }

    /// Asks the models for everything queued under `group`; the diagnostics
    /// of each rule, by rule id, in the order the asks were queued.
    fn flush(&self, _group: &str) -> Vec<(String, Vec<Diagnostic>)> {
        Vec::new()
    }
}

/// A run without models: nothing is bound.
pub struct NoModels;

impl Models for NoModels {}

/// The request that puts a subject, as `state`, to a model: one question per
/// decision, the one shape every asker of a model builds (a run's batch, a
/// teacher, the grading of a layer-1 model).
pub fn request_of<'a>(
    state: State,
    decisions: impl IntoIterator<Item = &'a DecisionRef>,
) -> SystemOneRequest {
    SystemOneRequest {
        state,
        model: String::new(),
        questions: decisions
            .into_iter()
            .map(|decision| (decision.id.clone(), Question::of(decision)))
            .collect(),
        options: serde_json::Map::new(),
    }
}

/// A probability of failing, made a verdict by the band: `Some(confidence)`
/// when the model fails the subject (P at or above `upper`), `None` when it
/// abstains. M1 has no calibration, so no lower bound clears a subject.
pub fn decide(noul: f64, upper: f64) -> Option<f64> {
    (noul >= upper).then(|| noul.max(1.0 - noul))
}
