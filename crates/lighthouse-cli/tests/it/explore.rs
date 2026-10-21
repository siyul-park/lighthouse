//! Exploring: a share of the subjects an attached model fails go on to the
//! next layer as if it had abstained. The attached model is the `command`
//! provider of the fake plugin, which fails `maybe_bad`.

use std::path::Path;

use lighthouse_model::{AgentKind, Attribution};
use lighthouse_store::{Change, RevisionSpec, append_revision};
use serde_json::{Value, json};

use crate::{
    attach::{DECISION, MODEL, attach, failures},
    models::{project, provider},
    models_plugin::{declared, json, label_table, lighthouse},
};

/// A project with the model attached and a share to explore.
fn exploring(share: &str) -> tempfile::TempDir {
    let models = declared().replace("[models]\n", &format!("[models]\nexplore = {share}\n"));
    let dir = project(&models);
    label_table(dir.path());
    assert_eq!(attach(dir.path(), &[]).status.code(), Some(0));
    dir
}

/// The reasons the check leaves subjects open for, one per subject.
fn reasons(dir: &Path) -> Vec<String> {
    let out = lighthouse(
        dir,
        &[
            "check",
            "--no-store",
            "--no-cache",
            "--format",
            "json",
            "--detail",
            "full",
        ],
    );
    let report: Value = json(&out);
    let mut found: Vec<String> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["decision"] == DECISION)
        .filter_map(|f| f["note"].as_str().map(str::to_owned))
        .collect();
    found.sort();
    found
}

#[test]
fn a_share_of_nothing_explores_nothing() {
    let dir = exploring("0.0");

    assert_eq!(failures(dir.path()), 2);
}

#[test]
fn a_share_of_everything_sends_every_fail_of_the_attached_model_on() {
    let dir = exploring("1.0");

    // The model's own fail on `maybe_bad` is set aside; `bad_one` is the
    // check's.
    assert_eq!(failures(dir.path()), 1);
    let named: Vec<String> = reasons(dir.path())
        .into_iter()
        .filter(|reason| reason.contains("explored"))
        .collect();
    assert_eq!(named.len(), 1, "{named:?}");
    assert!(named[0].contains(MODEL), "{named:?}");
    assert!(named[0].contains("said fail at"), "{named:?}");
}

#[test]
fn the_same_subjects_are_explored_every_run() {
    let dir = exploring("0.5");

    let first = (failures(dir.path()), reasons(dir.path()));

    assert_eq!(first, (failures(dir.path()), reasons(dir.path())));
}

#[test]
fn a_layer_below_that_decides_carries_what_the_explored_model_said() {
    // The project binds `fake/any` too, which fails what the attached model
    // fails; a revision written by hand attaches the model past the gate.
    let models = provider("command").replace("[models]\n", "[models]\nexplore = 1.0\n");
    let dir = project(&models);
    let spec = RevisionSpec {
        decision: DECISION.to_owned(),
        parent: None,
        meaning_version: None,
        check_revision: None,
        change: Change::Minor,
        evaluation: json!({}),
        approval: Attribution::new(AgentKind::Person, Some("kim".to_owned())),
        model: Some(MODEL.to_owned()),
        generated_at_time: "2026-10-11T00:00:00Z".to_owned(),
    };
    append_revision(dir.path(), spec).unwrap();

    let out = lighthouse(
        dir.path(),
        &[
            "check",
            "--no-store",
            "--no-cache",
            "--format",
            "json",
            "--detail",
            "full",
        ],
    );

    let report: Value = json(&out);
    let explored: Vec<&Value> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["decision"] == DECISION && f["evidence"]["explored"].is_object())
        .collect();
    assert_eq!(explored.len(), 1, "{report}");
    assert!(explored[0]["evidence"]["explored"]["confidence"].is_number());
    assert!(explored[0]["evidence"]["explored"]["model"].is_null());
    assert!(explored[0]["evidence"]["model"].is_null());
    let by = explored[0]["modelVersion"].as_str().unwrap();
    assert!(by.starts_with("fake/any@"), "{by}");
}
