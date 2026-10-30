//! Grading the model a revision attaches, laid over the run without it.

use lighthouse_ml::{
    eval::{EvalReport, grade},
    metrics::Outcome,
};
use lighthouse_model::{Judgment, Result};
use lighthouse_plugin::ModelClient;

use crate::layer_support::*;

#[test]
fn a_subject_the_model_fails_is_failed_and_the_rest_keep_the_outcome_they_had() {
    let root = tempfile::tempdir().unwrap();
    let run = base(&[("one", "a.rs", "bad"), ("two", "a.rs", "unsure")]);
    let labels = [
        label("one", Judgment::Fail, None, None),
        label("two", Judgment::Fail, None, None),
        label("three", Judgment::Pass, None, None),
        label(
            "example:x:1",
            Judgment::Pass,
            Some(Outcome::Pass),
            Some(subject("e.rs", "bad")),
        ),
        label(
            "example:x:2",
            Judgment::Fail,
            Some(Outcome::Review),
            Some(subject("e.rs", "bad")),
        ),
    ];
    let models = |_: &str| -> Result<Box<dyn ModelClient>> {
        Ok(Box::new(table(&[("bad", 0.95), ("unsure", 0.6)])))
    };
    let decision = decision();
    let layered = over("fake/m", root.path(), &decision, &models)
        .apply(&run, &labels)
        .unwrap();
    let outcome = |key: &str| {
        let label = layered.labels.iter().find(|l| l.key == key).unwrap();
        (label.observed, layered.found.outcome_of(label))
    };
    // Above the band the check uses: a fail. Below it: still for review.
    assert_eq!(outcome("one").1, Outcome::Fail);
    assert_eq!(outcome("two").1, Outcome::Review);
    // Unselected stays unselected; a pass an example shows stays a pass,
    // though the model would fail the subject; a review example is failed.
    assert_eq!(outcome("three").1, Outcome::NotApplicable);
    assert_eq!(outcome("example:x:1").0, Some(Outcome::Pass));
    assert_eq!(outcome("example:x:2").0, Some(Outcome::Fail));
    assert_eq!(
        layered
            .labels
            .iter()
            .filter(|l| l.observed.is_some())
            .count(),
        2
    );
    let info = layered.found.layer().unwrap();
    assert_eq!((info.model.as_str(), info.cross_fitted), ("fake/m", 0));
}

#[test]
fn a_layer_that_fails_what_a_person_passed_proposes_to_detach() {
    let root = tempfile::tempdir().unwrap();
    let decision = decision();
    let run = base(&[("one", "a.rs", "x"), ("two", "a.rs", "y")]);
    let labels = [
        label("one", Judgment::Pass, None, None),
        label("two", Judgment::Pass, None, None),
    ];
    let failing = |_: &str| -> Result<Box<dyn ModelClient>> {
        Ok(Box::new(table(&[("x", 0.95), ("y", 0.95)])))
    };
    let layered = over("fake/m", root.path(), &decision, &failing)
        .apply(&run, &labels)
        .unwrap();
    let graded = report(&labels, &layered);
    let proposal = graded.proposal.expect("precision 0 misses the policy");
    assert_eq!(proposal.model, "fake/m");
    assert!(
        proposal.reason.starts_with("precision 0.00 < 0.90"),
        "{}",
        proposal.reason
    );

    let right = [
        label("one", Judgment::Fail, None, None),
        label("two", Judgment::Fail, None, None),
    ];
    let layered = over("fake/m", root.path(), &decision, &failing)
        .apply(&run, &right)
        .unwrap();
    assert!(report(&right, &layered).proposal.is_none());
}

#[test]
fn a_report_without_a_layer_says_nothing_of_one() {
    let run = base(&[("one", "a.rs", "x")]);
    let labels = [label("one", Judgment::Fail, None, None)];
    let result = grade(RULE, None, None, policy(), &labels, &run, None);
    let graded = EvalReport::pool(vec![result]).unwrap();
    let json = serde_json::to_value(&graded).unwrap();
    assert!(json.get("proposal").is_none());
    assert!(json["current"].get("layer").is_none());
}
