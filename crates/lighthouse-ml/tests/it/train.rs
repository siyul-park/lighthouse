use std::{collections::BTreeMap, fs};

use lighthouse_ml::{
    labels::{Label, Source},
    metrics::Truth,
    train::{Rows, Target, TrainRequest, train},
};
use lighthouse_plugin::Subject;
use lighthouse_spec::Catalog;
use serde_json::json;

const RULE: &str = "core/file-size";

fn request() -> TrainRequest {
    TrainRequest {
        decision: RULE.to_owned(),
        model: None,
        from_model: None,
        baseline: None,
        repos: Vec::new(),
        config: None,
    }
}

fn label(key: &str, truth: Truth, source: Source) -> Label {
    Label {
        key: key.to_owned(),
        truth,
        weight: source.weight(),
        source,
        attribution: json!({"type": "Person"}),
        observed: None,
        subject: None,
    }
}

fn subject() -> Subject {
    serde_json::from_value(json!({"key": "k", "kind": "function", "id": "a", "language": "rust", "file": "a.rs", "text": "fn a() {}"}))
        .expect("a subject")
}

#[test]
fn rows_of_labels_with_a_subject_count_and_the_rest_are_unresolved() {
    let observed = BTreeMap::from([("seen".to_owned(), subject())]);
    let labels = [
        label("seen", Truth::Fail, Source::Person),
        label("gone", Truth::Pass, Source::Tsv),
        label("na", Truth::NotApplicable, Source::Person),
    ];
    let decision = Catalog::bundled().decision(RULE).unwrap().reference();
    let rows = Rows::of(&labels, &observed, None, &decision).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let target = Target {
        root: dir.path(),
        reference: None,
        decision: &decision,
        model: None,
    };
    let trained = train(&request(), vec![rows], &target).unwrap();
    assert_eq!(trained.subjects, 1);
    assert_eq!(trained.unresolved, 1);
    assert_eq!(trained.by_source["person"], 1);
    assert!(trained.result.is_none());
}

#[test]
fn train_writes_the_same_dataset_for_the_same_labels_and_no_job_without_a_model() {
    let decision = Catalog::bundled().decision(RULE).unwrap().reference();
    let dir = tempfile::tempdir().unwrap();
    let observed = BTreeMap::from([("seen".to_owned(), subject())]);
    let run = || {
        let rows = Rows::of(
            &[label("seen", Truth::Fail, Source::Person)],
            &observed,
            None,
            &decision,
        )
        .unwrap();
        let target = Target {
            root: dir.path(),
            reference: Some("local/m"),
            decision: &decision,
            model: None,
        };
        train(&request(), vec![rows], &target).unwrap()
    };
    let (first, second) = (run(), run());
    assert_eq!(first.training_file, second.training_file);
    assert_eq!(first.model, "local/m");
    assert!(first.training_file.starts_with("sha256:"));
    assert_eq!(
        fs::read_to_string(&first.dataset).unwrap().lines().count(),
        1
    );
}
