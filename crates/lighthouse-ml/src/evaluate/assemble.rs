//! Putting what each repository gave into one report.

use std::collections::BTreeSet;

use lighthouse_spec::Evaluation;

use crate::{
    Result,
    evaluate::{
        CAVEAT, EvalReport, EvalRequest, Found, LayerInfo, Proposal, Score, Side, write_audit,
    },
    metrics::{Metrics, Outcome, Truth, agreement, evaluate},
};

/// What one repository gave.
pub struct Part {
    pub(super) policy: Evaluation,
    pub(super) current: Vec<(Truth, Outcome)>,
    pub(super) candidate: Vec<(Truth, Outcome)>,
    /// The probabilities of failing models gave the labelled subjects.
    pub(super) scored: Vec<Score>,
    pub(super) candidate_scored: Vec<Score>,
    /// The notices of the run of each check.
    pub(super) notices: Vec<String>,
    pub(super) candidate_notices: Vec<String>,
    pub(super) pairs: Vec<(Outcome, Outcome)>,
    pub(super) decided: Usage,
    pub(super) candidate_decided: Usage,
    pub(super) audit: Vec<String>,
    pub(super) check: String,
    pub(super) candidate_check: Option<String>,
    /// The examples that should pass and the candidate newly fails.
    pub(super) regressions: Vec<String>,
    /// The layer each side was graded with, when it had one.
    pub(super) layer: Option<LayerInfo>,
    pub(super) candidate_layer: Option<LayerInfo>,
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
        let sum = |pick: fn(&Part) -> Usage| {
            self.parts
                .iter()
                .fold(Usage::default(), |a, p| a.plus(pick(p)))
        };
        let pool_scored = |pick: fn(&Part) -> &Vec<Score>| -> Vec<Score> {
            self.parts.iter().flat_map(|p| pick(p).clone()).collect()
        };
        let pool_notices = |pick: fn(&Part) -> &Vec<String>| -> BTreeSet<String> {
            self.parts.iter().flat_map(|p| pick(p).clone()).collect()
        };
        let current_side = side(
            first.check.clone(),
            &current,
            pool_scored(|p| &p.scored),
            pool_notices(|p| &p.notices),
            sum(|p| p.decided),
            pool_layer(&self.parts, |p| &p.layer),
        );
        let candidate_side = with_candidate.then(|| {
            let pairs = pool(|p| &p.candidate);
            side(
                first.candidate_check.clone().unwrap_or_default(),
                &pairs,
                pool_scored(|p| &p.candidate_scored),
                pool_notices(|p| &p.candidate_notices),
                sum(|p| p.candidate_decided),
                pool_layer(&self.parts, |p| &p.candidate_layer),
            )
        });
        let judged = candidate_side.as_ref().unwrap_or(&current_side);
        let meets_policy = judged
            .metrics
            .precision
            .map(|_| shortfall(&judged.metrics, policy).is_none());
        let proposal = current_side.layer.as_ref().and_then(|layer| {
            Some(Proposal {
                action: "detach",
                model: layer.model.clone(),
                reason: shortfall(&current_side.metrics, policy)?,
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
            decided: pairs
                .iter()
                .filter(|(was, now)| {
                    *was == Outcome::Review && matches!(now, Outcome::Fail | Outcome::Pass)
                })
                .count(),
            regressions: self
                .parts
                .iter()
                .flat_map(|p| p.regressions.clone())
                .collect(),
            caveat: CAVEAT,
            audit,
            proposal,
        })
    }
}

/// How a check misses the policy by the point estimates of its precision and,
/// when the policy sets a target, of its recall; none when it meets it (or
/// has no precision to judge).
pub(crate) fn shortfall(metrics: &Metrics, policy: Evaluation) -> Option<String> {
    let precision = metrics.precision?;
    if precision.value < policy.precision {
        return Some(format!(
            "precision {:.2} < {:.2} (n={})",
            precision.value, policy.precision, precision.of
        ));
    }
    let target = policy.recall?;
    match metrics.recall {
        Some(recall) if recall.value >= target => None,
        Some(recall) => Some(format!(
            "recall {:.2} < {target:.2} (n={})",
            recall.value, recall.of
        )),
        None => Some(format!("recall unknown < {target:.2}")),
    }
}

/// One check's numbers over the pooled pairs and scores.
fn side(
    check: String,
    pairs: &[(Truth, Outcome)],
    mut scores: Vec<Score>,
    notices: BTreeSet<String>,
    used: Usage,
    layer: Option<LayerInfo>,
) -> Side {
    scores.sort_by(|a, b| (&a.repo, &a.fingerprint).cmp(&(&b.repo, &b.fingerprint)));
    let scored: Vec<(Truth, f64)> = scores
        .iter()
        .map(|s| {
            let truth = if s.violates { Truth::Fail } else { Truth::Pass };
            (truth, s.probability)
        })
        .collect();
    Side {
        check,
        metrics: evaluate(pairs, &scored),
        model_decided: used.decided,
        reviews: used.reviews,
        calls: used.calls,
        cost: used.cost,
        scores,
        notices: notices.into_iter().collect(),
        layer,
    }
}

/// The layer of every repository as one: the labels it cross-fitted added up
/// and the models of its refits together.
fn pool_layer(parts: &[Part], pick: fn(&Part) -> &Option<LayerInfo>) -> Option<LayerInfo> {
    let mut layers = parts.iter().filter_map(|p| pick(p).as_ref());
    let mut pooled = layers.next()?.clone();
    for more in layers {
        pooled.cross_fitted += more.cross_fitted;
        pooled.refits.extend(more.refits.iter().cloned());
    }
    pooled.refits.sort();
    pooled.refits.dedup();
    Some(pooled)
}
