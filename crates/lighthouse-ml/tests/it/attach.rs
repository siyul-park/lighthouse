//! Whether the evaluation of a model allows attaching it, and the revisions
//! that attach and detach.

use lighthouse_ml::{
    attach::{attachable, evidence},
    evaluate::{EvalReport, Layer, Side, Usage},
    metrics::{Metrics, Outcome},
};
use lighthouse_model::Judgment;
use lighthouse_spec::Evaluation;

type Pairs = [(Judgment, Outcome)];

const FAIL: Judgment = Judgment::Fail;

fn side(pairs: &Pairs) -> Side {
    Side {
        check: "local/rule".to_owned(),
        metrics: Metrics::of(pairs, &[]),
        usage: Usage::default(),
        scores: Vec::new(),
        notices: Vec::new(),
        layer: None,
    }
}

/// A report that allows attaching: the model fails the one violation the
/// check left for review.
fn allowed() -> EvalReport {
    EvalReport {
        decision: "local/rule".to_owned(),
        labelled: 2,
        policy: Evaluation::default(),
        current: side(&[(FAIL, Outcome::Fail), (FAIL, Outcome::Review)]),
        candidate: Some(side(&[(FAIL, Outcome::Fail), (FAIL, Outcome::Fail)])),
        agreement: None,
        meets_policy: Some(true),
        decided: 1,
        regressions: Vec::new(),
        audit: None,
        proposal: None,
    }
}

#[test]
fn a_model_that_meets_the_policy_and_loses_nothing_may_be_attached() {
    assert_eq!(attachable(&allowed()), Vec::<String>::new());
}

#[test]
fn a_model_that_misses_the_policy_is_refused_with_the_shortfall() {
    let report = EvalReport {
        meets_policy: Some(false),
        candidate: Some(side(&[
            (Judgment::Pass, Outcome::Fail),
            (FAIL, Outcome::Fail),
        ])),
        ..allowed()
    };
    let reasons = attachable(&report);
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(
        reasons[0].contains("misses the policy: precision 0.50 < 0.90"),
        "{reasons:?}"
    );
}

#[test]
fn a_model_with_no_precision_to_judge_is_refused() {
    let report = EvalReport {
        meets_policy: None,
        ..allowed()
    };
    let reasons = attachable(&report);
    assert!(reasons[0].contains("no precision"), "{reasons:?}");
}

#[test]
fn a_model_that_loses_recall_is_refused() {
    let report = EvalReport {
        current: side(&[(FAIL, Outcome::Fail), (FAIL, Outcome::Fail)]),
        candidate: Some(side(&[(FAIL, Outcome::Fail), (FAIL, Outcome::Review)])),
        ..allowed()
    };
    assert_eq!(
        attachable(&report),
        vec!["recall falls from 1.00 to 0.50".to_owned()]
    );
}

#[test]
fn a_model_that_newly_fails_an_example_that_should_pass_is_refused() {
    let report = EvalReport {
        regressions: vec!["fp1".to_owned(), "fp2".to_owned()],
        ..allowed()
    };
    assert_eq!(
        attachable(&report),
        vec!["examples that should pass newly fail: fp1, fp2".to_owned()]
    );
}

#[test]
fn a_model_that_decides_nothing_the_check_leaves_for_review_is_refused() {
    let report = EvalReport {
        decided: 0,
        ..allowed()
    };
    let reasons = attachable(&report);
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].contains("decides no labelled subject"));
}

#[test]
fn every_reason_is_given_at_once() {
    let report = EvalReport {
        meets_policy: Some(false),
        candidate: Some(side(&[
            (Judgment::Pass, Outcome::Fail),
            (FAIL, Outcome::Review),
        ])),
        regressions: vec!["fp1".to_owned()],
        decided: 0,
        ..allowed()
    };
    assert_eq!(attachable(&report).len(), 4);
}

#[test]
fn an_evaluation_that_graded_no_model_allows_nothing() {
    let report = EvalReport {
        candidate: None,
        ..allowed()
    };
    assert_eq!(
        attachable(&report),
        vec!["the evaluation graded no model".to_owned()]
    );
}

#[test]
fn the_evidence_is_the_numbers_of_the_model_and_the_dataset_it_learned_from() {
    let model = "fake/learned@sha256:abc";
    let mut report = allowed();
    report.candidate.as_mut().unwrap().layer = Some(Layer {
        model: model.to_owned(),
        cross_fitted: 2,
        refits: Vec::new(),
        training_file: Some("sha256:def".to_owned()),
    });

    let value = evidence(&report).unwrap();
    assert_eq!(value["labelled"], 2);
    assert_eq!(value["policy"]["precision"], 0.9);
    assert_eq!(value["precision"]["hits"], 2);
    assert_eq!(value["recall"]["hits"], 2);
    assert_eq!(value["coverage"]["of"], 2);
    assert_eq!(value["crossFitted"], 2);
    assert_eq!(value["trainingFile"], "sha256:def");

    report.candidate.as_mut().unwrap().layer = None;
    let alone = evidence(&report).unwrap();
    assert_eq!(alone["trainingFile"], serde_json::Value::Null);
}
