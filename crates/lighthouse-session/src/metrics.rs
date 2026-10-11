//! The metrics of evaluating a check against labels, with Wilson 95%
//! intervals. A proportion over nothing is `None`, never zero.

use serde::Serialize;

/// z for a 95% interval.
const Z: f64 = 1.959_963_984_540_054;

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
        let n = of as f64;
        let p = hits as f64 / n;
        let z2 = Z * Z;
        let centre = (p + z2 / (2.0 * n)) / (1.0 + z2 / n);
        let half = Z * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt() / (1.0 + z2 / n);
        Some(Self {
            hits,
            of,
            value: p,
            low: (centre - half).max(0.0),
            high: (centre + half).min(1.0),
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
    /// The check did not select it.
    Unselected,
}

/// What a labelled subject was: what the label says, and how the check came
/// out. A `notApplicable` label is about the selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    Fail,
    Pass,
    NotApplicable,
}

/// The metrics, each `None` where there is nothing to divide.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Metrics {
    pub labelled: usize,
    pub precision: Option<Rate>,
    pub fpr: Option<Rate>,
    pub recall: Option<Rate>,
    pub fnr: Option<Rate>,
    /// The share of selected subjects not judged `notApplicable`; kept apart
    /// from conformance.
    pub selector_precision: Option<Rate>,
    /// The share of selected subjects the check decided.
    pub coverage: Option<Rate>,
    pub abstention: Option<Rate>,
}

/// Evaluates `pairs` of (truth, outcome). An unselected `fail` is a missed
/// violation; an unselected `pass` is a correct silence. A subject left to
/// review is not decided, so it is in neither precision nor recall, only in
/// coverage.
pub fn evaluate(pairs: &[(Truth, Outcome)]) -> Metrics {
    let count = |want: &dyn Fn(&(Truth, Outcome)) -> bool| pairs.iter().filter(|p| want(p)).count();
    let selected = |p: &(Truth, Outcome)| p.1 != Outcome::Unselected;
    let tp = count(&|p| *p == (Truth::Fail, Outcome::Fail));
    let fp = count(&|p| *p == (Truth::Pass, Outcome::Fail));
    let tn = count(&|p| p.0 == Truth::Pass && matches!(p.1, Outcome::Pass | Outcome::Unselected));
    let fn_ = count(&|p| p.0 == Truth::Fail && matches!(p.1, Outcome::Pass | Outcome::Unselected));
    let chosen = count(&|p| selected(p));
    let not_applicable = count(&|p| selected(p) && p.0 == Truth::NotApplicable);
    let reviewed = count(&|p| p.1 == Outcome::Review);
    Metrics {
        labelled: pairs.len(),
        precision: Rate::new(tp, tp + fp),
        fpr: Rate::new(fp, fp + tn),
        recall: Rate::new(tp, tp + fn_),
        fnr: Rate::new(fn_, tp + fn_),
        selector_precision: Rate::new(chosen - not_applicable, chosen),
        coverage: Rate::new(chosen - reviewed, chosen),
        abstention: Rate::new(reviewed, chosen),
    }
}

/// The share of subjects two checks answer the same way.
pub fn agreement(pairs: &[(Outcome, Outcome)]) -> Option<Rate> {
    Rate::new(pairs.iter().filter(|(a, b)| a == b).count(), pairs.len())
}
