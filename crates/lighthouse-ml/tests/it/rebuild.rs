use std::{collections::BTreeMap, fs, path::Path};

use lighthouse_ml::{
    labels::{Label, Source},
    rebuild::{Learned, Status, unrebuildable},
    train::Rows,
};
use lighthouse_model::Judgment;
use lighthouse_plugin::{DecisionRef, Subject};
use lighthouse_protocol::{canonical, hash::sha256};
use serde_json::json;

use crate::layer_support::{Memorizer, manifest};

const RULE: &str = "core/max-lines";

fn decision() -> DecisionRef {
    lighthouse_test_support::bundled()
        .decision(RULE)
        .unwrap()
        .reference()
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
            judgment: Judgment::Fail,
            source: Source::Person,
            attribution: json!({"type": "Person"}),
            observed: None,
            subject: None,
        })
        .collect();
    let rows = Rows::of(&labels, &observed, None, &decision()).unwrap();
    rows.write(root, &decision()).unwrap()
}

#[test]
fn learned_names_the_trainer_and_the_digest_of_a_model_that_is_a_file() {
    let model = Learned::parse("local/learned@sha256:abc").unwrap();

    assert_eq!(model.trainer, "local/learned");
    assert_eq!(model.hex, "abc");
    assert!(Learned::parse("openai/gpt-5").is_none());
    assert!(Learned::parse("local/learned@v2").is_none());
}

#[test]
fn check_finds_the_artifact_present_when_its_content_has_the_digest_in_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let document = json!({"card": {}, "seen": ["a"]});
    let hex = sha256(canonical(&document));
    let reference = format!("fake/m@sha256:{hex}");
    let model = Learned::parse(&reference).unwrap();
    assert_eq!(model.check(dir.path()).unwrap(), None);

    let store = dir.path().join(".lighthouse/models/sha256");
    fs::create_dir_all(&store).unwrap();
    fs::write(store.join(format!("{hex}.json")), canonical(&document)).unwrap();
    assert_eq!(
        model.check(dir.path()).unwrap(),
        Some((Status::Present, None))
    );

    let other = json!({"card": {}, "seen": ["b"]});
    fs::write(store.join(format!("{hex}.json")), canonical(&other)).unwrap();
    let (status, note) = model.check(dir.path()).unwrap().unwrap();
    assert_eq!(status, Status::Differs);
    assert!(note.unwrap().contains(&sha256(canonical(&other))));
}

#[test]
fn rows_write_the_same_digest_for_the_same_rows() {
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
    model.capabilities.training = None;
    let reason = unrebuildable(&Memorizer(model)).unwrap();
    assert!(reason.contains("does not learn"), "{reason}");
}

#[test]
fn rebuild_makes_the_model_again_only_when_the_digest_is_the_one_named() {
    let dir = tempfile::tempdir().unwrap();
    let digest = dataset(dir.path(), &["a", "b"]);
    let trainer = Memorizer(manifest(true));
    let request = |hex: &str| {
        let reference = format!("fake/m@sha256:{hex}");
        Learned::parse(&reference)
            .unwrap()
            .rebuild(&trainer, dir.path(), RULE, &digest, || Ok(digest.clone()))
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
fn rebuild_says_when_the_labels_changed_since_the_dataset_was_attached() {
    let dir = tempfile::tempdir().unwrap();
    let attached = format!("sha256:{}", "0".repeat(64));
    let today = dataset(dir.path(), &["a"]);
    let reference = format!("fake/m@sha256:{}", "1".repeat(64));
    let (status, detail) = Learned::parse(&reference)
        .unwrap()
        .rebuild(
            &Memorizer(manifest(true)),
            dir.path(),
            RULE,
            &attached,
            || Ok(today.clone()),
        )
        .unwrap();

    assert_eq!(status, Status::LabelsChanged);
    assert!(detail.unwrap().contains("labels changed since attach"));
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
