//! The metrics of evaluating a check against labels, with Wilson 95%
//! intervals. A proportion over nothing is `None`, never zero.

use lighthouse_model::Judgment;
use lighthouse_protocol::stats::wilson;
use lighthouse_spec::Evaluation;
use serde::Serialize;

/// The equal-width bins of P(fail) the expected calibration error uses.
const BINS: usize = 10;

/// A proportion with its Wilson 95% interval.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rate {
    pub hits: usize,
    pub of: usize,
    pub value: f64,
    pub low: f64,
    pub high: f64,
}

impl Rate {
    /// `hits` out of `of`; `None` when `of` is zero.
    pub fn new(hits: usize, of: usize) -> Option<Self> {
        if of == 0 {
            return None;
        }
        let (low, high) = wilson(hits as f64, of as f64);
        Some(Self {
            hits,
            of,
            value: hits as f64 / of as f64,
            low,
            high,
        })
    }
}

/// How a check came out for one labelled subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Reported as a violation.
    Fail,
    /// The check selected it and let it pass.
    Pass,
    /// Nothing decided it: left to review.
    Review,
    /// The check did not select it (SARIF `notApplicable`).
    NotApplicable,
}

/// The metrics, each `None` where there is nothing to divide.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub labelled: usize,
    pub precision: Option<Rate>,
    pub fpr: Option<Rate>,
    pub recall: Option<Rate>,
    pub fnr: Option<Rate>,
    /// The share of selected subjects not judged `notApplicable`; kept apart
    /// from conformance.
    pub selector_precision: Option<Rate>,
    /// The share of selected subjects the check decided; its complement is the
    /// share left to review.
    pub coverage: Option<Rate>,
    /// Labelled subjects a model gave a probability of failing; the rest
    /// abstained and are counted, not scored.
    pub answered: usize,
    /// Mean (P(fail) - y)^2 over the answered subjects, y = 1 for a `fail`
    /// (scikit-learn's `brier_score_loss`).
    pub brier: Option<f64>,
    /// The probability that a `fail` is given a higher P(fail) than a `pass`,
    /// ties counting half (scikit-learn's `roc_auc_score`); `None` when one
    /// class is missing.
    pub auroc: Option<f64>,
    /// Expected calibration error over the answered subjects: the bins of P(fail)
    /// weigh the gap between the share of fails and the mean P(fail)
    /// (Guo et al., ICML 2017).
    pub ece: Option<f64>,
}

impl Metrics {
    /// The metrics of `pairs` of (judgment, outcome), and of the probabilities
    /// of failing `scored` (violates, P(fail)) that models gave the labelled
    /// subjects; a `notApplicable` label is about the selector and is not
    /// scored. An unselected `fail` is a missed violation; an unselected
    /// `pass` is a correct silence. A subject labelled `fail` that was left to
    /// review was not found, so it is a miss (recall counts it); a subject left
    /// to review is in no precision, and in coverage only.
    pub fn of(pairs: &[(Judgment, Outcome)], scored: &[(bool, f64)]) -> Self {
        let count =
            |want: &dyn Fn(&(Judgment, Outcome)) -> bool| pairs.iter().filter(|p| want(p)).count();
        let selected = |p: &(Judgment, Outcome)| p.1 != Outcome::NotApplicable;
        let tp = count(&|p| *p == (Judgment::Fail, Outcome::Fail));
        let fp = count(&|p| *p == (Judgment::Pass, Outcome::Fail));
        let tn = count(&|p| {
            p.0 == Judgment::Pass && matches!(p.1, Outcome::Pass | Outcome::NotApplicable)
        });
        let missed = count(&|p| {
            p.0 == Judgment::Fail
                && matches!(
                    p.1,
                    Outcome::Pass | Outcome::Review | Outcome::NotApplicable
                )
        });
        let chosen = count(&|p| selected(p));
        let not_applicable = count(&|p| selected(p) && p.0 == Judgment::NotApplicable);
        let reviewed = count(&|p| p.1 == Outcome::Review);
        Self {
            labelled: pairs.len(),
            precision: Rate::new(tp, tp + fp),
            fpr: Rate::new(fp, fp + tn),
            recall: Rate::new(tp, tp + missed),
            fnr: Rate::new(missed, tp + missed),
            selector_precision: Rate::new(chosen - not_applicable, chosen),
            coverage: Rate::new(chosen - reviewed, chosen),
            answered: scored.len(),
            brier: brier(scored),
            auroc: auroc(scored),
            ece: ece(scored),
        }
    }

    /// How the check misses the policy by the point estimates of its precision
    /// and, when the policy sets a target, of its recall; none when it meets it
    /// (or has no precision to judge).
    pub fn shortfall(&self, policy: Evaluation) -> Option<String> {
        let precision = self.precision?;
        if precision.value < policy.precision {
            return Some(format!(
                "precision {:.2} < {:.2} (n={})",
                precision.value, policy.precision, precision.of
            ));
        }
        let target = policy.recall?;
        match self.recall {
            Some(recall) if recall.value >= target => None,
            Some(recall) => Some(format!(
                "recall {:.2} < {target:.2} (n={})",
                recall.value, recall.of
            )),
            None => Some(format!("recall unknown < {target:.2}")),
        }
    }
}

/// The mean squared distance of P(fail) from the label, `true` for a fail.
fn brier(scored: &[(bool, f64)]) -> Option<f64> {
    let sum: f64 = scored
        .iter()
        .map(|&(fail, p)| (p - f64::from(u8::from(fail))).powi(2))
        .sum();
    (!scored.is_empty()).then(|| sum / scored.len() as f64)
}

/// The Mann-Whitney statistic over every (fail, pass) pair.
fn auroc(scored: &[(bool, f64)]) -> Option<f64> {
    let of = |want: bool| -> Vec<f64> {
        scored
            .iter()
            .filter(|&&(fail, _)| fail == want)
            .map(|&(_, p)| p)
            .collect()
    };
    let (fails, passes) = (of(true), of(false));
    if fails.is_empty() || passes.is_empty() {
        return None;
    }
    let wins: f64 = fails
        .iter()
        .flat_map(|f| passes.iter().map(move |p| (f, p)))
        .map(|(f, p)| match f.total_cmp(p) {
            std::cmp::Ordering::Greater => 1.0,
            std::cmp::Ordering::Equal => 0.5,
            std::cmp::Ordering::Less => 0.0,
        })
        .sum();
    Some(wins / (fails.len() * passes.len()) as f64)
}

/// Σ_b (n_b / N) · |mean(y)_b - mean(p)_b| over [`BINS`] equal-width bins,
/// which is Σ_b |fails_b - Σp_b| / N.
fn ece(scored: &[(bool, f64)]) -> Option<f64> {
    let mut bins = [(0usize, 0.0f64, 0.0f64); BINS];
    for &(fail, p) in scored {
        let at = ((p * BINS as f64) as usize).min(BINS - 1);
        let (n, fails, sum) = bins[at];
        bins[at] = (n + 1, fails + f64::from(u8::from(fail)), sum + p);
    }
    let gap: f64 = bins
        .iter()
        .map(|&(_, fails, sum)| (fails - sum).abs() / scored.len() as f64)
        .sum();
    (!scored.is_empty()).then_some(gap)
}
