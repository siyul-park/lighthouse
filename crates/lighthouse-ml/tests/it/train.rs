use std::{collections::BTreeMap, fs};

use lighthouse_ml::{
    labels::{Sample, Source},
    train::{Fit, Rows, train},
};
use lighthouse_model::Judgment;
use lighthouse_plugin::{DATASETS_DIR, Subject, dataset_path};
use serde_json::json;

const RULE: &str = "core/max-lines";

fn label(key: &str, judgment: Judgment, source: Source) -> Sample {
    Sample {
        key: key.to_owned(),
        judgment,
        source,
        attribution: json!({"type": "Person"}),
        observed: None,
        subject: None,
    }
}

fn subject() -> Subject {
    serde_json::from_value(json!({"key": "k", "kind": "function", "id": "a", "state": {"language": "rust", "file": "a.rs", "text": "fn a() {}", "facts": null, "neighbors": []}}))
        .expect("a subject")
}

#[test]
fn rows_of_labels_with_a_subject_count_and_the_rest_are_unresolved() {
    let observed = BTreeMap::from([("seen".to_owned(), subject())]);
    let labels = [
        label("seen", Judgment::Fail, Source::Person),
        label("gone", Judgment::Pass, Source::Tsv),
        label("na", Judgment::NotApplicable, Source::Person),
    ];
    let decision = lighthouse_test_support::bundled()
        .decision(RULE)
        .unwrap()
        .reference();
    let rows = Rows::of(&labels, &observed, None, &decision).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let fit = Fit {
        root: dir.path(),
        reference: None,
        decision: &decision,
        model: None,
    };
    let trained = train(rows, &fit).unwrap();
    assert_eq!(trained.subjects, 1);
    assert_eq!(trained.unresolved, 1);
    assert_eq!(trained.by_source["person"], 1);
    assert!(trained.result.is_none());
}

#[test]
fn examples_are_not_rows_because_the_question_already_shows_them() {
    let observed = BTreeMap::from([("seen".to_owned(), subject()), ("ex".to_owned(), subject())]);
    let labels = [
        label("seen", Judgment::Fail, Source::Person),
        label("ex", Judgment::Pass, Source::Example),
    ];
    let decision = lighthouse_test_support::bundled()
        .decision(RULE)
        .unwrap()
        .reference();
    let rows = Rows::of(&labels, &observed, None, &decision).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let fit = Fit {
        root: dir.path(),
        reference: None,
        decision: &decision,
        model: None,
    };
    let trained = train(rows, &fit).unwrap();
    assert_eq!(trained.subjects, 1);
    assert_eq!(trained.unresolved, 0);
    assert!(!trained.by_source.contains_key("example"));
}

#[test]
fn train_writes_the_same_dataset_for_the_same_labels_and_no_job_without_a_model() {
    let decision = lighthouse_test_support::bundled()
        .decision(RULE)
        .unwrap()
        .reference();
    let dir = tempfile::tempdir().unwrap();
    let observed = BTreeMap::from([("seen".to_owned(), subject())]);
    let run = || {
        let rows = Rows::of(
            &[label("seen", Judgment::Fail, Source::Person)],
            &observed,
            None,
            &decision,
        )
        .unwrap();
        let fit = Fit {
            root: dir.path(),
            reference: Some("local/m"),
            decision: &decision,
            model: None,
        };
        train(rows, &fit).unwrap()
    };
    let (first, second) = (run(), run());
    assert_eq!(first.training_file, second.training_file);
    assert_eq!(first.model, "local/m");
    assert!(first.training_file.starts_with("sha256:"));
    let hex = first.training_file.strip_prefix("sha256:").unwrap();
    let file = dataset_path(&dir.path().join(DATASETS_DIR), hex);
    assert_eq!(fs::read_to_string(file).unwrap().lines().count(), 1);
}

#[test]
fn rows_extend_adds_what_another_repository_gave() {
    let decision = lighthouse_test_support::bundled()
        .decision(RULE)
        .unwrap()
        .reference();
    let observed = BTreeMap::from([("seen".to_owned(), subject())]);
    let mut rows = Rows::of(
        &[label("seen", Judgment::Fail, Source::Person)],
        &observed,
        None,
        &decision,
    )
    .unwrap();
    let more = Rows::of(
        &[label("gone", Judgment::Pass, Source::Person)],
        &observed,
        None,
        &decision,
    )
    .unwrap();
    rows.extend(more);

    let dir = tempfile::tempdir().unwrap();
    let fit = Fit {
        root: dir.path(),
        reference: None,
        decision: &decision,
        model: None,
    };
    let trained = train(rows, &fit).unwrap();
    assert_eq!((trained.subjects, trained.unresolved), (1, 1));
}
