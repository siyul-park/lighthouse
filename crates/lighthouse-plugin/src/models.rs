//! Models as the rules see them: a [`Classifier`] is a plugin's model, and
//! [`Models`] is what a run offers a rule to put subjects to the models its
//! decision is bound to. The wire types are the protocol's: a model is trained
//! on what it is later asked about.

pub use lighthouse_protocol::{
    Answer, Capabilities, CreateFineTuningJobRequest, DATASETS_DIR, DecisionRef, Example,
    FineTuneJobError, FineTuneMethod, FineTuneSupervisedHyperparameters, FineTuneSupervisedMethod,
    FineTuningJob, FineTuningJobStatus, LineKeys, ListModelResponse, MODELS_DIR, Model, Neighbor,
    Question, State, Subject, Supports, SystemOneRequest, SystemOneResponse, TrainingLine,
    artifact_path, dataset_path,
};

use lighthouse_model::Diagnostic;

use crate::Error;

/// The probability at which a model's answer is a `fail`; a model never
/// clears a subject.
pub const FAIL_AT: f64 = 0.9;

/// A model offered by a plugin.
pub trait Classifier: Send + Sync {
    fn manifest(&self) -> &Model;
    /// Answers every subject of the request, or fails as a whole.
    fn classify(&self, params: &SystemOneRequest) -> Result<SystemOneResponse, Error>;
    /// Runs a fine-tuning job; only a model whose manifest supports `fine_tuning` does.
    /// The job is final: `succeeded` or `failed`.
    fn train(&self, _request: &CreateFineTuningJobRequest) -> Result<FineTuningJob, Error> {
        Err(Error::model(&self.manifest().id, "does not train"))
    }
}

/// Subjects of one decision to be judged.
#[derive(Debug, Clone)]
pub struct Ask {
    /// The rule of the decision.
    pub rule: String,
    /// Changes whenever the way the rule judges changes; cached answers of
    /// another revision are not used.
    pub revision: String,
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
    /// The attached model that failed it too and was set aside for the sake of
    /// exploring.
    pub explored: Option<Explored>,
}

/// A fail of an attached model that was treated as an abstention, to find out
/// what the layers below would say.
#[derive(Debug, Clone, PartialEq)]
pub struct Explored {
    /// The model, `<provider>/<model>@<version>`.
    pub model: String,
    /// max(p, 1 - p) of the probability it gave.
    pub confidence: f64,
}

/// Why a subject was left undecided: it is a candidate for agent review.
#[derive(Debug, Clone, PartialEq)]
pub enum Abstained {
    /// Every model of the route abstained or was unsure.
    Undecided,
    /// No model is bound to the decision, or models are off.
    Unbound,
    /// The run's budget was spent.
    OverBudget,
    /// A model failed; the reason.
    Failed(String),
    /// The attached model failed it and no layer below decided otherwise.
    Explored(Explored),
}

impl std::fmt::Display for Abstained {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undecided => f.write_str("no model decided"),
            Self::Unbound => f.write_str("no model is bound"),
            Self::OverBudget => f.write_str("the model budget was spent"),
            Self::Failed(reason) => write!(f, "a model failed: {reason}"),
            Self::Explored(Explored { model, confidence }) => {
                write!(f, "explored: {model} said fail at {confidence:.2}")
            }
        }
    }
}

/// The models' answer for one subject.
#[derive(Debug, Clone, PartialEq)]
pub enum Answered {
    Decided(Decided),
    Abstained(Abstained),
}

/// How a queued ask turns into diagnostics once its subjects are answered.
pub type Settle = Box<dyn FnOnce(Vec<Answered>) -> Vec<Diagnostic> + Send + Sync>;

/// What a run offers rules that ask models.
pub trait Models: Send + Sync {
    /// Whether the run keeps the subjects rules put to it, for training.
    fn collecting(&self) -> bool {
        false
    }

    /// Whether the run puts the subjects `rule` suspects to models: a model is
    /// bound to its decision, or the run asks for model work. Otherwise the
    /// rule skips its `abstain` route.
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
            .map(|_| Answered::Abstained(Abstained::Unbound))
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
            .map(|decision| (decision.id.clone(), Question::new(decision)))
            .collect(),
        options: serde_json::Map::new(),
    }
}

/// `Some(confidence)` when a model that gave `p` fails the subject (`p` at or
/// above [`FAIL_AT`]), `None` when it abstains: no lower bound clears a subject.
pub fn fails(p: f64) -> Option<f64> {
    (p >= FAIL_AT).then(|| p.max(1.0 - p))
}
