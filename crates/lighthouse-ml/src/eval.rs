//! `lighthouse eval`: how a decision's check does on the labels recorded for
//! it, and how a candidate check does beside it.
//!
//! The labelled subjects are found again by running the checks on the
//! repositories the labels come from. A subject a check did not report is
//! taken as one it did not select (or let pass); the labels come mostly from
//! flagged findings, so recall is a bound, not a measurement: sample unflagged
//! subjects for a person to label to measure it.

use std::{iter::Sum, path::PathBuf};

use lighthouse_spec::Evaluation;
use schemars::JsonSchema;
use serde::Serialize;

use crate::metrics::{Metrics, Rate};

mod found;
mod grade;
mod layer;

pub use found::{Candidate, Found};
pub use grade::{RepoResult, grade};
pub use layer::{Overlaid, Overlay};

/// The result of an evaluation.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvalReport {
    pub decision: String,
    pub labelled: usize,
    pub policy: Evaluation,
    pub current: Side,
    pub candidate: Option<Side>,
    /// How often the candidate answers as the current check does.
    pub agreement: Option<Rate>,
    /// Whether the check (the candidate, when there is one) meets the
    /// evaluation policy by the point estimates of its precision and, when the
    /// policy sets a target, of its recall; the intervals are shown beside.
    pub meets_policy: Option<bool>,
    /// Labelled subjects the candidate decides that the current check leaves
    /// for review.
    pub decided: usize,
    /// The examples that should pass and the candidate newly fails.
    pub regressions: Vec<String>,
    /// The table of unflagged subjects written for a person to label.
    pub audit: Option<PathBuf>,
    /// What to do about an attached model that misses the policy: detach it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Proposal>,
}

/// One check's numbers.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Side {
    pub check: String,
    pub metrics: Metrics,
    #[serde(flatten)]
    pub usage: Usage,
    /// The probability each labelled subject was given, by repository and
    /// fingerprint: the raw answers the metrics are computed from, so that
    /// reports can be pooled.
    pub scores: Vec<Score>,
    /// What went wrong with the models, or was worth saying: the notices of
    /// the run, so that a failing model is not silent.
    pub notices: Vec<String>,
    /// The model attached by revision this side is graded with, and how.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<Layer>,
}

/// What a model gave a labelled subject.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Score {
    /// The label's fingerprint.
    pub fingerprint: String,
    pub repo: Option<String>,
    /// Whether the label says the subject violates the requirement.
    pub violates: bool,
    /// P(fail) the model gave.
    pub probability: f64,
    /// The model that gave it: `<reference>@<version>`.
    pub model: String,
}

/// How a layer was graded, for the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub model: String,
    /// Labelled subjects the model learned from, graded by refits.
    pub cross_fitted: usize,
    /// The models the refits made.
    pub refits: Vec<String>,
    /// The dataset (`sha256:<hex>`) the model learned from, when its artifact
    /// is in the store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub training_file: Option<String>,
}

/// What a check asked of models.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Subjects a model decided.
    pub model_decided: usize,
    /// Subjects left to review.
    pub reviews: usize,
    /// Questions put to models, and what they cost.
    #[serde(flatten)]
    pub usage: lighthouse_model::Usage,
}

impl Sum for Usage {
    fn sum<I: Iterator<Item = Self>>(usages: I) -> Self {
        usages.fold(Self::default(), |sum, usage| Self {
            model_decided: sum.model_decided + usage.model_decided,
            reviews: sum.reviews + usage.reviews,
            usage: lighthouse_model::Usage {
                calls: sum.usage.calls + usage.usage.calls,
                cost: sum.usage.cost + usage.usage.cost,
            },
        })
    }
}

/// The monitor proposes to detach a model; nothing is written until it is
/// done.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Proposal {
    pub model: String,
    pub reason: String,
}
