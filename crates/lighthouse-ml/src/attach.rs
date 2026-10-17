//! Attaching a model to a decision by revision (`lighthouse model attach`):
//! whether the evaluation of the model allows it, and what justifies it. The
//! revision itself is the caller's to write.

use lighthouse_model::Result;
use serde_json::{Value, json};

use crate::evaluate::EvalReport;

/// Why the model an evaluation graded may not be attached: none when it may.
/// The candidate side of `report` is the check with the model as its first
/// layer, the current side the check without it.
pub fn attachable(report: &EvalReport) -> Vec<String> {
    let Some(candidate) = &report.candidate else {
        return vec!["the evaluation graded no model".to_owned()];
    };
    let mut reasons = Vec::new();
    if report.meets_policy != Some(true) {
        reasons.push(match candidate.metrics.shortfall(report.policy) {
            Some(missed) => format!("it misses the policy: {missed}"),
            None => "it has no precision to judge against the policy".to_owned(),
        });
    }
    match (report.current.metrics.recall, candidate.metrics.recall) {
        (Some(current), Some(with)) if with.value < current.value => reasons.push(format!(
            "recall falls from {:.2} to {:.2}",
            current.value, with.value
        )),
        (Some(current), None) => reasons.push(format!(
            "recall is unknown where it was {:.2}",
            current.value
        )),
        _ => {}
    }
    if !report.regressions.is_empty() {
        reasons.push(format!(
            "examples that should pass newly fail: {}",
            report.regressions.join(", ")
        ));
    }
    if report.decided == 0 {
        reasons.push("it decides no labelled subject the check leaves for review".to_owned());
    }
    reasons
}

/// What justifies attaching the model the evaluation graded: its numbers, with
/// the dataset it learned from. A revision records it; a refusal shows it.
pub fn evidence(report: &EvalReport) -> Result<Value> {
    let candidate = report.candidate.as_ref().ok_or("nothing was graded")?;
    let metrics = &candidate.metrics;
    Ok(json!({
        "labelled": report.labelled,
        "policy": report.policy,
        "precision": metrics.precision,
        "recall": metrics.recall,
        "coverage": metrics.coverage,
        "crossFitted": candidate.layer.as_ref().map_or(0, |layer| layer.cross_fitted),
        "trainingFile": candidate.layer.as_ref().and_then(|layer| layer.training_file.as_ref()),
    }))
}
