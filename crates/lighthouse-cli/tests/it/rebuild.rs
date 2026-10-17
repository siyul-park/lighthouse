//! `model rebuild` through the command line: a learned model is a file kept
//! out of git, and a clone that has the revision that attached it makes the
//! file again from the labels. The trainer is the `fake-model` plugin, which
//! is deterministic and names its artifact by the digest of its content.

use std::{fs, path::Path};

use lighthouse_model::{AgentKind, Attribution};
use lighthouse_store::{Change, RevisionSpec, append_revision};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{
    cache_fixtures::write,
    models::project,
    models_plugin::{RPC, json, label_table, lighthouse, with_plugin},
};

const DECISION: &str = "local/naming-ban";

/// A project that trained `fake/m1` on the labels and attached the result,
/// and that model's reference.
fn attached() -> (TempDir, String) {
    let dir = project(&RPC.replace("classification = \"fake/m1\"\n", ""));
    with_plugin(dir.path());
    spread(dir.path());
    label_table(dir.path());
    let trained = lighthouse(
        dir.path(),
        &[
            "model",
            "train",
            DECISION,
            "--model",
            "fake/m1",
            "--labels",
            "labels.tsv",
            "--format",
            "json",
        ],
    );
    let made = json(&trained)["result"]["result_files"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let model = format!("fake/m1@{made}");
    let out = lighthouse(
        dir.path(),
        &[
            "model",
            "attach",
            DECISION,
            "--labels",
            "labels.tsv",
            "--model",
            &model,
            "--attributed-to",
            "person:kim",
            "--format",
            "json",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    (dir, model)
}

/// The labelled functions in files of their own, which are the groups the
/// cross-fit leaves out one at a time.
fn spread(dir: &Path) {
    write(
        dir,
        "src/lib.rs",
        "mod b;\nmod c;\npub fn bad_one() {}\npub fn maybe_good() {}\npub fn fine() {}\n",
    );
    write(dir, "src/b.rs", "pub fn maybe_bad() {}\n");
    write(dir, "src/c.rs", "pub fn maybe_unsure() {}\n");
}

fn rebuild(dir: &Path, extra: &[&str]) -> (Option<i32>, Value) {
    let mut args = vec!["model", "rebuild", "--format", "json"];
    args.extend_from_slice(extra);
    let out = lighthouse(dir, &args);
    (out.status.code(), json(&out))
}

fn artifact(dir: &Path, model: &str) -> std::path::PathBuf {
    let hex = model.split_once("@sha256:").unwrap().1;
    dir.join(format!(".lighthouse/models/sha256/{hex}.json"))
}

/// The dataset files of the project.
fn datasets(dir: &Path) -> Vec<std::path::PathBuf> {
    let found = fs::read_dir(dir.join(".lighthouse/datasets/sha256")).unwrap();
    found.map(|entry| entry.unwrap().path()).collect()
}

#[test]
fn a_model_whose_file_is_here_is_present() {
    let (dir, model) = attached();

    let (code, rows) = rebuild(dir.path(), &[]);
    assert_eq!(code, Some(0));
    assert_eq!(
        rows,
        json!([{"decision": DECISION, "model": model, "status": "present"}])
    );
    let text = lighthouse(dir.path(), &["model", "rebuild"]);
    let shown = String::from_utf8_lossy(&text.stdout);
    assert_eq!(shown, format!("{DECISION}\t{model}\tpresent\n"));
}

#[test]
fn a_missing_file_is_trained_again_to_the_same_digest_from_the_stored_dataset() {
    let (dir, model) = attached();
    let path = artifact(dir.path(), &model);
    let before = fs::read_to_string(&path).unwrap();
    fs::remove_file(&path).unwrap();

    let (code, rows) = rebuild(dir.path(), &[]);

    assert_eq!(code, Some(0), "{rows}");
    assert_eq!(rows[0]["status"], "rebuilt", "{rows}");
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn a_missing_file_and_dataset_are_made_again_from_the_same_labels() {
    let (dir, model) = attached();
    fs::remove_file(artifact(dir.path(), &model)).unwrap();
    for dataset in datasets(dir.path()) {
        fs::remove_file(dataset).unwrap();
    }

    let (code, rows) = rebuild(dir.path(), &["--labels", "labels.tsv"]);

    assert_eq!(code, Some(0), "{rows}");
    assert_eq!(rows[0]["status"], "rebuilt", "{rows}");
    assert!(artifact(dir.path(), &model).is_file());
    assert_eq!(datasets(dir.path()).len(), 1);
}

#[test]
fn labels_that_changed_since_attach_train_nothing() {
    let (dir, model) = attached();
    fs::remove_file(artifact(dir.path(), &model)).unwrap();
    for dataset in datasets(dir.path()) {
        fs::remove_file(dataset).unwrap();
    }
    let labels = fs::read_to_string(dir.path().join("labels.tsv")).unwrap();
    let changed: Vec<&str> = labels.lines().take(2).collect();
    write(
        dir.path(),
        "labels.tsv",
        &format!("{}\n", changed.join("\n")),
    );

    let (code, rows) = rebuild(dir.path(), &["--labels", "labels.tsv"]);

    assert_eq!(code, Some(1), "{rows}");
    assert_eq!(rows[0]["status"], "labels-changed", "{rows}");
    let detail = rows[0]["detail"].as_str().unwrap();
    assert!(
        detail.contains("labels changed since attach (dataset sha256:"),
        "{detail}"
    );
    assert!(detail.ends_with("train and attach again"), "{detail}");
    assert!(!artifact(dir.path(), &model).exists());
}

#[test]
fn a_file_that_is_not_what_its_name_says_differs_and_stays() {
    let (dir, model) = attached();
    let path = artifact(dir.path(), &model);
    fs::write(&path, "{\"card\":{}}").unwrap();

    let (code, rows) = rebuild(dir.path(), &[]);

    assert_eq!(code, Some(1), "{rows}");
    assert_eq!(rows[0]["status"], "differs", "{rows}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\"card\":{}}");
}

#[test]
fn a_model_that_is_a_service_is_not_local() {
    let (dir, _) = attached();
    let spec = RevisionSpec {
        decision: DECISION.to_owned(),
        parent: None,
        meaning_version: None,
        check_revision: None,
        change: Change::Minor,
        evaluation: json!({}),
        approval: Attribution::new(AgentKind::Person, Some("kim".to_owned())),
        model: Some("openai/gpt".to_owned()),
        generated_at_time: "2026-10-11T00:00:00Z".to_owned(),
    };
    append_revision(dir.path(), spec).unwrap();

    let (code, rows) = rebuild(dir.path(), &[]);

    assert_eq!(code, Some(0), "{rows}");
    assert_eq!(rows[0]["status"], "not-local", "{rows}");
    assert_eq!(rows[0]["model"], "openai/gpt");
}
