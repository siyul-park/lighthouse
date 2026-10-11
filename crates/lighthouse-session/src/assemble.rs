//! Putting what each repository gave into one report.

use lighthouse_spec::{Evaluation, Tolerate};

use crate::{
    Result,
    evaluate::{CAVEAT, EvalReport, EvalRequest, Found, Side, write_audit},
    metrics::{Outcome, Truth, agreement, evaluate},
};

/// What one repository gave.
pub(crate) struct Part {
    pub(crate) policy: Evaluation,
    pub(crate) current: Vec<(Truth, Outcome)>,
    pub(crate) candidate: Vec<(Truth, Outcome)>,
    pub(crate) pairs: Vec<(Outcome, Outcome)>,
    pub(crate) decided: Usage,
    pub(crate) candidate_decided: Usage,
    pub(crate) audit: Vec<String>,
    pub(crate) check: String,
    pub(crate) candidate_check: Option<String>,
}

/// What a check asked of models.
#[derive(Clone, Copy, Default)]
pub(crate) struct Usage {
    pub(crate) decided: usize,
    pub(crate) reviews: usize,
    pub(crate) calls: usize,
    pub(crate) cost: f64,
}

impl Usage {
    pub(crate) fn of(found: &Found) -> Self {
        Self {
            decided: found.model_decided,
            reviews: found.reviews,
            calls: found.calls,
            cost: found.cost,
        }
    }

    pub(crate) fn plus(self, other: Self) -> Self {
        Self {
            decided: self.decided + other.decided,
            reviews: self.reviews + other.reviews,
            calls: self.calls + other.calls,
            cost: self.cost + other.cost,
        }
    }
}

#[derive(Default)]
pub(crate) struct Gathered {
    pub(crate) parts: Vec<Part>,
}

impl Gathered {
    pub(crate) fn add(&mut self, part: Part) {
        self.parts.push(part);
    }

    pub(crate) fn finish(self, request: &EvalRequest, with_candidate: bool) -> Result<EvalReport> {
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
        let meets_policy = match policy.tolerate {
            Tolerate::Fn => judged.metrics.precision,
            Tolerate::Fp => judged.metrics.recall,
        }
        .map(|rate| rate.low >= policy.precision);
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
