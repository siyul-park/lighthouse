//! A model fitted on labels is graded on them by refits that leave each fold
//! out.

use std::fs;

use lighthouse_ml::{
    eval::Found,
    labels::Label,
    train::{Fit, Rows, train},
};
use lighthouse_model::{Judgment, Result};
use lighthouse_plugin::{DecisionRef, ModelClient};

use crate::layer_support::*;

#[test]
fn a_model_that_fits_its_labels_is_graded_on_them_by_refits() {
    let root = tempfile::tempdir().unwrap();
    let decision = decision();
    // Three files of two subjects each, every one a violation.
    let places: Vec<(String, String, String)> = ["a.rs", "b.rs", "c.rs"]
        .iter()
        .flat_map(|file| {
            (0..2).map(move |n| {
                (
                    format!("{file}:{n}"),
                    (*file).to_owned(),
                    format!("text of {file}:{n}"),
                )
            })
        })
        .collect();
    let run = base(
        &places
            .iter()
            .map(|(f, file, text)| (f.as_str(), file.as_str(), text.as_str()))
            .collect::<Vec<_>>(),
    );
    let labels: Vec<Label> = places
        .iter()
        .map(|(f, _, _)| label(f, Judgment::Fail, None, None))
        .collect();
    let trainer = Memorizer(manifest(true));
    let trained = train(
        Rows::of(&labels, run.subjects(), None, &decision).unwrap(),
        &Fit {
            root: root.path(),
            reference: Some("fake/learned"),
            decision: &decision,
            model: Some(&trainer),
        },
    )
    .unwrap();
    let served = trained.result.unwrap().fine_tuned_model.unwrap();
    let reference = format!("fake/{served}");
    let digest = served.split_once("@sha256:").unwrap().1.to_owned();

    let models = |name: &str| -> Result<Box<dyn ModelClient>> {
        Ok(match name.split_once("@sha256:") {
            Some((_, hex)) => Box::new(remembered(root.path(), hex)),
            None => Box::new(Memorizer(manifest(true))),
        })
    };
    // As it is, the model is perfect on what it learned.
    let memory = remembered(root.path(), &digest);
    let asked = |text: &str| {
        let state = run
            .subjects()
            .values()
            .find(|s| s.state.text == text)
            .unwrap()
            .state
            .clone();
        memory
            .classify(&lighthouse_plugin::request_of(state, [&decision]))
            .unwrap()
            .answers[RULE]
            .noul
    };
    assert!(places.iter().all(|(_, _, text)| asked(text) == 1.0));

    let layered = over(&reference, root.path(), &decision, &models)
        .apply(&run, &labels)
        .unwrap();
    let graded = report(&labels, &layered);
    // Each label is graded by a refit that never saw its file: nothing is
    // remembered, so nothing is failed.
    let info = graded.current.layer.as_ref().unwrap();
    assert_eq!(info.cross_fitted, 6);
    assert_eq!(info.refits.len(), 3);
    let recall = graded.current.metrics.recall.unwrap();
    assert_eq!((recall.hits, recall.of), (0, 6));
    // The refits were scratch: only the model and its dataset are left.
    let left = |dir: &str| {
        fs::read_dir(root.path().join(dir).join("sha256"))
            .unwrap()
            .count()
    };
    assert_eq!(
        (left(".lighthouse/models"), left(".lighthouse/datasets")),
        (1, 1)
    );
}

#[test]
fn a_trainer_that_is_not_deterministic_cannot_refit() {
    let (root, run, labels, reference, decision) = learned(false);
    let models = |name: &str| -> Result<Box<dyn ModelClient>> {
        Ok(match name.split_once("@sha256:") {
            Some((_, hex)) => Box::new(remembered(root.path(), hex)),
            None => Box::new(Memorizer(manifest(false))),
        })
    };
    let error = over(&reference, root.path(), &decision, &models)
        .apply(&run, &labels)
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("its trainer is not deterministic"),
        "{error}"
    );
    assert!(
        error.contains("label more and grade on new labels"),
        "{error}"
    );
}

#[test]
fn a_dataset_the_store_lost_is_named() {
    let (root, run, labels, reference, decision) = learned(true);
    for entry in fs::read_dir(root.path().join(".lighthouse/datasets/sha256")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let models = |name: &str| -> Result<Box<dyn ModelClient>> {
        Ok(match name.split_once("@sha256:") {
            Some((_, hex)) => Box::new(remembered(root.path(), hex)),
            None => Box::new(Memorizer(manifest(true))),
        })
    };
    let error = over(&reference, root.path(), &decision, &models)
        .apply(&run, &labels)
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("are not in .lighthouse/datasets; train it again"),
        "{error}"
    );
    assert!(
        error.contains("the labels `fake/learned@sha256:"),
        "{error}"
    );
}

/// A model trained on two labelled subjects: the root, the run, the labels,
/// the model's reference and the decision.
fn learned(deterministic: bool) -> (tempfile::TempDir, Found, Vec<Label>, String, DecisionRef) {
    let root = tempfile::tempdir().unwrap();
    let decision = decision();
    let run = base(&[("one", "a.rs", "first"), ("two", "b.rs", "second")]);
    let labels = vec![
        label("one", Judgment::Fail, None, None),
        label("two", Judgment::Fail, None, None),
    ];
    let trainer = Memorizer(manifest(deterministic));
    let trained = train(
        Rows::of(&labels, run.subjects(), None, &decision).unwrap(),
        &Fit {
            root: root.path(),
            reference: Some("fake/learned"),
            decision: &decision,
            model: Some(&trainer),
        },
    )
    .unwrap();
    let served = trained.result.unwrap().fine_tuned_model.unwrap();
    (root, run, labels, format!("fake/{served}"), decision)
}
