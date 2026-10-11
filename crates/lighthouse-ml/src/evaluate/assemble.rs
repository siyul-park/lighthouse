//! Putting what each repository gave into one report.

use lighthouse_spec::Evaluation;

use crate::{
    Result,
    evaluate::{CAVEAT, EvalReport, EvalRequest, Found, Side, write_audit},
    metrics::{Outcome, Truth, agreement, evaluate},
};

/// What one repository gave.
pub struct Part {
    pub(super) policy: Evaluation,
    pub(super) current: Vec<(Truth, Outcome)>,
    pub(super) candidate: Vec<(Truth, Outcome)>,
    pub(super) pairs: Vec<(Outcome, Outcome)>,
    pub(super) decided: Usage,
    pub(super) candidate_decided: Usage,
    pub(super) audit: Vec<String>,
    pub(super) check: String,
    pub(super) candidate_check: Option<String>,
}

/// What a check asked of models.
#[derive(Clone, Copy, Default)]
pub(super) struct Usage {
    pub(super) decided: usize,
    pub(super) reviews: usize,
    pub(super) calls: usize,
    pub(super) cost: f64,
}

impl Usage {
    pub(super) fn of(found: &Found) -> Self {
        Self {
            decided: found.model_decided,
            reviews: found.reviews,
            calls: found.calls,
            cost: found.cost,
        }
    }

    pub(super) fn plus(self, other: Self) -> Self {
        Self {
            decided: self.decided + other.decided,
            reviews: self.reviews + other.reviews,
            calls: self.calls + other.calls,
            cost: self.cost + other.cost,
        }
    }
}

/// What every repository gave so far.
#[derive(Default)]
pub struct Gathered {
    parts: Vec<Part>,
}

impl Gathered {
    /// Takes what one more repository gave.
    pub fn add(&mut self, part: Part) {
        self.parts.push(part);
    }

    /// The report over every repository; `with_candidate` says a candidate
    /// ran beside the current check.
    pub fn finish(self, request: &EvalRequest, with_candidate: bool) -> Result<EvalReport> {
        let first = self.parts.first().ok_or("nothing to evaluate")?;
        let policy = first.policy;
        let pool = |pick: fn(&Part) -> &Vec<(Truth, Outcome)>| -> Vec<(Truth, Outcome)> {
            self.parts.iter().flat_map(|p| pick(p).clone()).collect()
        };
        let current = pool(|p| &p.current);
        let side = |check: String, pairs: &[(Truth, Outcome)], used: Usage| Side {
            check,
            metrics: evaluate(pairs),
            model_decided: used.decided,
            reviews: used.reviews,
            calls: used.calls,
            cost: used.cost,
        };
        let sum = |pick: fn(&Part) -> Usage| {
            self.parts
                .iter()
                .fold(Usage::default(), |a, p| a.plus(pick(p)))
        };
        let current_side = side(first.check.clone(), &current, sum(|p| p.decided));
        let candidate_side = with_candidate.then(|| {
            let pairs = pool(|p| &p.candidate);
            side(
                first.candidate_check.clone().unwrap_or_default(),
                &pairs,
                sum(|p| p.candidate_decided),
            )
        });
        let judged = candidate_side.as_ref().unwrap_or(&current_side);
        let meets_policy = judged.metrics.precision.map(|precision| {
            precision.value >= policy.precision
                && policy.recall.is_none_or(|target| {
                    judged
                        .metrics
                        .recall
                        .is_some_and(|recall| recall.value >= target)
                })
        });
        let audit = write_audit(
            request,
            self.parts.iter().flat_map(|p| p.audit.clone()).collect(),
        )?;
        let pairs: Vec<(Outcome, Outcome)> =
            self.parts.iter().flat_map(|p| p.pairs.clone()).collect();
        Ok(EvalReport {
            decision: request.decision.clone(),
            labelled: current.len(),
            policy,
            agreement: with_candidate.then(|| agreement(&pairs)).flatten(),
            current: current_side,
            candidate: candidate_side,
            meets_policy,
            caveat: CAVEAT,
            audit,
        })
    }
}
