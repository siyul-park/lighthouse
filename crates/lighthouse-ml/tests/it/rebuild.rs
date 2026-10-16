use std::{collections::BTreeMap, fs, path::Path};

use lighthouse_ml::{
    labels::{Label, Source},
    metrics::Truth,
    rebuild::{
        Artifact, Learned, Refit, Status, artifact, has_dataset, learned, refit, unrebuildable,
    },
    train::{Rows, write_dataset},
};
use lighthouse_model::{canonical, hash::sha256};
use lighthouse_plugin::{DecisionRef, Subject};
use lighthouse_spec::Catalog;
use serde_json::json;

use crate::layer_support::{Memorizer, manifest};

const RULE: &str = "core/file-size";

fn decision() -> DecisionRef {
    Catalog::bundled().decision(RULE).unwrap().reference()
}

fn dataset(root: &Path, texts: &[&str]) -> String {
    let observed: BTreeMap<String, Subject> = texts
        .iter()
        .map(|text| {
            let subject: Subject = serde_json::from_value(json!({"key": text, "kind": "function", "id": text, "state": {"language": "rust", "file": format!("{text}.rs"), "text": text, "facts": null, "neighbors": []}})).unwrap();
            ((*text).to_owned(), subject)
        })
        .collect();
    let labels: Vec<Label> = texts
        .iter()
        .map(|text| Label {
            key: (*text).to_owned(),
            truth: Truth::Fail,
            weight: 1.0,
            source: Source::Person,
            attribution: json!({"type": "Person"}),
            observed: None,
            subject: None,
        })
        .collect();
    let rows = Rows::of(&labels, &observed, None, &decision()).unwrap();
    write_dataset(vec![rows], root, &decision()).unwrap()
}

#[test]
fn learned_names_the_trainer_and_the_digest_of_a_model_that_is_a_file() {
    let model = learned("local/learned@sha256:abc").unwrap();

    assert_eq!(model.trainer, "local/learned");
    assert_eq!(model.hex, "abc");
    assert!(learned("openai/gpt-5").is_none());
    assert!(learned("local/learned@v2").is_none());
}

#[test]
fn artifact_is_present_when_its_content_has_the_digest_in_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let document = json!({"card": {}, "seen": ["a"]});
    let hex = sha256(canonical(&document));
    assert_eq!(artifact(dir.path(), &hex).unwrap(), Artifact::Missing);

    let store = dir.path().join(".lighthouse/models/sha256");
    fs::create_dir_all(&store).unwrap();
    fs::write(store.join(format!("{hex}.json")), canonical(&document)).unwrap();
    assert_eq!(artifact(dir.path(), &hex).unwrap(), Artifact::Present);

    let other = json!({"card": {}, "seen": ["b"]});
    fs::write(store.join(format!("{hex}.json")), canonical(&other)).unwrap();
    let found = artifact(dir.path(), &hex).unwrap();
    assert_eq!(found, Artifact::Differs(sha256(canonical(&other))));
}

#[test]
fn has_dataset_looks_for_the_digest_in_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let digest = dataset(dir.path(), &["a", "b"]);

    assert!(has_dataset(dir.path(), &digest));
    assert!(!has_dataset(
        dir.path(),
        &format!("sha256:{}", "0".repeat(64))
    ));
}

#[test]
fn write_dataset_gives_the_same_digest_for_the_same_rows() {
    let dir = tempfile::tempdir().unwrap();

    let first = dataset(dir.path(), &["a", "b"]);

    assert_eq!(first, dataset(dir.path(), &["a", "b"]));
    assert_ne!(first, dataset(dir.path(), &["a"]));
}

#[test]
fn a_trainer_must_learn_and_make_the_same_model_to_make_it_again() {
    let mut model = manifest(true);
    assert_eq!(unrebuildable(&Memorizer(model.clone())), None);

    model.deterministic = false;
    let reason = unrebuildable(&Memorizer(model.clone())).unwrap();
    assert!(reason.contains("not deterministic"), "{reason}");

    model.deterministic = true;
    model.capabilities.fine_tuning = None;
    let reason = unrebuildable(&Memorizer(model)).unwrap();
    assert!(reason.contains("does not learn"), "{reason}");
}

#[test]
fn refit_makes_the_model_again_only_when_the_digest_is_the_one_named() {
    let dir = tempfile::tempdir().unwrap();
    let digest = dataset(dir.path(), &["a", "b"]);
    let trainer = Memorizer(manifest(true));
    let request = |hex: &str| {
        let model = Learned {
            trainer: "fake/m",
            hex,
        };
        refit(
            &trainer,
            &Refit {
                root: dir.path(),
                decision: RULE,
                model: &model,
                dataset: &digest,
            },
        )
        .unwrap()
    };

    let (status, detail) = request(&"0".repeat(64));
    assert_eq!(status, Status::Differs);
    let detail = detail.unwrap();
    let made = detail
        .strip_prefix("training made sha256:")
        .and_then(|rest| rest.split(';').next())
        .unwrap();

    assert_eq!(request(made), (Status::Rebuilt, None));
}

#[test]
fn only_a_model_that_is_there_or_made_again_or_a_service_holds() {
    assert!(Status::Present.holds());
    assert!(Status::Rebuilt.holds());
    assert!(Status::NotLocal.holds());
    assert!(!Status::Differs.holds());
    assert!(!Status::LabelsChanged.holds());
    assert!(!Status::NotRebuildable.holds());
}
