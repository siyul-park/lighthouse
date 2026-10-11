//! Models as the rules see them: a [`Classifier`] is a plugin's model, and
//! [`Models`] is what a run offers a rule to put subjects to the models its
//! decision is bound to. The wire types are the protocol's: a model is trained
//! on what it is later asked about.

pub use lighthouse_protocol::{
    Answer, ClassifyParams, ClassifyResult, DecisionRef, LabelSource, ModelManifest, ModelRef,
    Neighbor, ResultKind, Shot, Subject, SubjectContext, TrainParams, TrainResult, TrainSubject,
};

use crate::Error;

/// A model offered by a plugin.
pub trait Classifier: Send + Sync {
    fn manifest(&self) -> &ModelManifest;
    /// Answers every subject of the request, or fails as a whole.
    fn classify(&self, params: &ClassifyParams) -> Result<ClassifyResult, Error>;
    /// Fits the model on labelled subjects; only a model whose manifest lists
    /// the `train` method does.
    fn train(&self, params: &TrainParams) -> Result<TrainResult, Error>;
}

/// Subjects of one decision to be judged.
#[derive(Debug, Clone)]
pub struct Ask {
    /// The rule of the decision.
    pub rule: String,
    /// Changes whenever the way the rule judges changes; cached answers of
    /// another revision are not used.
    pub revision: String,
    /// The least confidence at which an answer decides; below it, or abstaining,
    /// the next model of the cascade is asked.
    pub threshold: f64,
    pub params: ClassifyParams,
}

/// A subject a model decided.
#[derive(Debug, Clone, PartialEq)]
pub struct Decided {
    /// `Fail` or `Pass`, never `Review`.
    pub kind: ResultKind,
    pub confidence: f64,
    pub reason: String,
    /// The model that decided.
    pub model: ModelRef,
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

/// What a run offers rules that ask models.
pub trait Models: Send + Sync {
    /// Whether any model is bound to the rule's decision and models are on.
    fn bound(&self, rule: &str) -> bool;
    /// Whether the run keeps the subjects rules put to it, for training.
    fn collecting(&self) -> bool {
        false
    }
    /// Keeps subjects seen.
    fn observe(&self, _subjects: Vec<Subject>) {}
    /// One outcome per subject of `ask`, in order.
    fn classify(&self, ask: &Ask) -> Vec<Judged>;
}

/// A run without models: nothing is bound.
pub struct NoModels;

impl Models for NoModels {
    fn bound(&self, _: &str) -> bool {
        false
    }

    fn classify(&self, ask: &Ask) -> Vec<Judged> {
        ask.params
            .subjects
            .iter()
            .map(|_| Judged::Open(Open::Unbound))
            .collect()
    }
}
