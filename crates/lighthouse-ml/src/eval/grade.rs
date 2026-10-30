//! Grading the labels of a repository against what its checks found, and
//! putting what each repository gave into one report.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::{Judgment, Result};
use lighthouse_spec::Evaluation;

use super::{Candidate, EvalReport, Found, Layer, Proposal, Score, Side, Usage};
use crate::{
    labels::Sample,
    metrics::{Metrics, Outcome, Rate},
};

/// What one check gave in one repository.
struct SideInput {
    check: String,
    pairs: Vec<(Judgment, Outcome)>,
    /// The probabilities of failing models gave the labelled subjects.
    scores: Vec<Score>,
    /// The notices of the run of the check.
    notices: Vec<String>,
    usage: Usage,
    /// The layer the check was graded with, when it had one.
    layer: Option<Layer>,
}

/// What one repository gave.
pub struct RepoResult {
    policy: Evaluation,
    current: SideInput,
    candidate: Option<SideInput>,
    /// The current and the candidate outcome of each label.
    pairs: Vec<(Outcome, Outcome)>,
    /// The examples that should pass and the candidate newly fails.
    regressions: Vec<String>,
    /// The rows of the audit table for this repository.
    pub audit: Vec<String>,
}

/// What a repository gave: the labels graded by the outcome of the current
/// check of `decision` and, when there is one, of the candidate beside it.
/// `audit` asks for that many rows of selected subjects the check did not
/// flag.
pub fn grade(
    decision: &str,
    audit: Option<usize>,
    repo: Option<&str>,
    policy: Evaluation,
    labels: &[Sample],
    current: &Found,
    candidate: Option<Candidate>,
) -> RepoResult {
    let mut result = RepoResult {
        policy,
        current: SideInput {
            check: decision.to_owned(),
            pairs: labels
                .iter()
                .map(|l| (l.judgment, current.outcome_of(l)))
                .collect(),
            scores: labels
                .iter()
                .filter_map(|l| current.scored(current, l, repo))
                .collect(),
            notices: current.notices.clone(),
            usage: current.usage,
            layer: current.layer.clone(),
        },
        candidate: None,
        pairs: Vec::new(),
        regressions: Vec::new(),
        audit: Vec::new(),
    };
    let Some(candidate) = candidate else {
        result.audit = audit.map_or_else(Vec::new, |n| current.audit_rows(n, repo));
        return result;
    };
    let shown: BTreeMap<&str, Outcome> = candidate
        .labels
        .iter()
        .filter_map(|l| Some((l.key.as_str(), l.observed?)))
        .collect();
    let found = candidate.found;
    let mut beside = SideInput {
        check: candidate.id,
        pairs: Vec::new(),
        scores: Vec::new(),
        notices: found.notices.clone(),
        usage: found.usage,
        layer: found.layer.clone(),
    };
    for label in labels {
        let was = current.outcome_of(label);
        let now = if label.observed.is_some() {
            shown
                .get(label.key.as_str())
                .copied()
                .unwrap_or(Outcome::NotApplicable)
        } else {
            current
                .place
                .get(&label.key)
                .and_then(|place| found.by_place.get(place))
                .copied()
                .unwrap_or(Outcome::NotApplicable)
        };
        if label.observed.is_some()
            && label.judgment == Judgment::Pass
            && now == Outcome::Fail
            && was != Outcome::Fail
        {
            result.regressions.push(label.key.clone());
        }
        beside.pairs.push((label.judgment, now));
        beside.scores.extend(found.scored(current, label, repo));
        result.pairs.push((was, now));
    }
    result.audit = audit.map_or_else(Vec::new, |n| found.audit_rows(n, repo));
    result.candidate = Some(beside);
    result
}

impl EvalReport {
    /// The report over every repository. The table of unflagged subjects is
    /// the caller's to write, from the `audit` rows of the results.
    pub fn pool(results: Vec<RepoResult>) -> Result<Self> {
        let first = results.first().ok_or("nothing to evaluate")?;
        let policy = first.policy;
        let current = Side::pool(&results.iter().map(|r| &r.current).collect::<Vec<_>>());
        let candidate = first.candidate.is_some().then(|| {
            let beside: Vec<&SideInput> = results
                .iter()
                .filter_map(|r| r.candidate.as_ref())
                .collect();
            Side::pool(&beside)
        });
        let judged = candidate.as_ref().unwrap_or(&current);
        let meets_policy = judged
            .metrics
            .precision
            .map(|_| judged.metrics.shortfall(policy).is_none());
        let proposal = current.layer.as_ref().and_then(|layer| {
            Some(Proposal {
                model: layer.model.clone(),
                reason: current.metrics.shortfall(policy)?,
            })
        });
        let pairs: Vec<(Outcome, Outcome)> = results
            .iter()
            .flat_map(|r| r.pairs.iter().copied())
            .collect();
        let agreement = candidate
            .as_ref()
            .and_then(|_| Rate::new(pairs.iter().filter(|(a, b)| a == b).count(), pairs.len()));
        let decided = pairs
            .iter()
            .filter(|(was, now)| {
                *was == Outcome::Review && matches!(now, Outcome::Fail | Outcome::Pass)
            })
            .count();
        Ok(Self {
            decision: current.check.clone(),
            labelled: current.metrics.labelled,
            policy,
            agreement,
            current,
            candidate,
            meets_policy,
            decided,
            regressions: results.into_iter().flat_map(|r| r.regressions).collect(),
            audit: None,
            proposal,
        })
    }
}

impl Side {
    /// One check's numbers over the pooled pairs and scores of every
    /// repository.
    fn pool(inputs: &[&SideInput]) -> Self {
        let pairs: Vec<(Judgment, Outcome)> = inputs
            .iter()
            .flat_map(|i| i.pairs.iter().copied())
            .collect();
        let mut scores: Vec<Score> = inputs
            .iter()
            .flat_map(|i| i.scores.iter().cloned())
            .collect();
        scores.sort_by(|a, b| (&a.repo, &a.fingerprint).cmp(&(&b.repo, &b.fingerprint)));
        let scored: Vec<(bool, f64)> = scores.iter().map(|s| (s.violates, s.probability)).collect();
        let notices: BTreeSet<&String> = inputs.iter().flat_map(|i| &i.notices).collect();
        Self {
            check: inputs.first().map(|i| i.check.clone()).unwrap_or_default(),
            metrics: Metrics::of(&pairs, &scored),
            usage: inputs.iter().map(|i| i.usage).sum(),
            scores,
            notices: notices.into_iter().cloned().collect(),
            layer: pool_layer(inputs),
        }
    }
}

/// The layer of every repository as one: the labels it cross-fitted added up
/// and the models of its refits together.
fn pool_layer(inputs: &[&SideInput]) -> Option<Layer> {
    let mut layers = inputs.iter().filter_map(|i| i.layer.as_ref());
    let mut pooled = layers.next()?.clone();
    for more in layers {
        pooled.cross_fitted += more.cross_fitted;
        pooled.refits.extend(more.refits.iter().cloned());
    }
    pooled.refits.sort();
    pooled.refits.dedup();
    Some(pooled)
}
