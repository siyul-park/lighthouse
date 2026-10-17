//! A model attached to a decision by a revision, and detached again, through
//! the command line. The model is the `command` provider of the fake plugin:
//! it fails `maybe_bad`, passes `maybe_good` and is unsure of the rest.

use std::path::Path;

use lighthouse_store::{Change, Records};
use serde_json::Value;

use crate::{
    cache_fixtures::write,
    models::{count, project, results},
    models_plugin::{declared, fingerprints, json, label_table, lighthouse},
};

pub(crate) const DECISION: &str = "local/naming-ban";
pub(crate) const MODEL: &str = "fake/learned@sha256:abc";

pub(crate) fn attach(dir: &Path, extra: &[&str]) -> std::process::Output {
    let mut args = vec![
        "model",
        "attach",
        DECISION,
        "--labels",
        "labels.tsv",
        "--model",
        MODEL,
        "--attributed-to",
        "person:kim",
        "--format",
        "json",
    ];
    args.extend_from_slice(extra);
    lighthouse(dir, &args)
}

fn detach(dir: &Path) -> std::process::Output {
    lighthouse(dir, &["model", "detach", DECISION, "--format", "json"])
}

/// How many of the decision's results a check reports as failures.
pub(crate) fn failures(dir: &Path) -> usize {
    let out = lighthouse(
        dir,
        &["check", "--no-store", "--no-cache", "--format", "sarif"],
    );
    count(&results(&json(&out)), "fail")
}

/// A table that calls what the model fails, `maybe_bad`, a false alarm.
pub(crate) fn mislabelled(dir: &Path) {
    let fp = fingerprints(dir);
    write(
        dir,
        "labels.tsv",
        &format!(
            "fingerprint\trepo\tlocation\tverdict\treason\n{}\tr\tx\tTP\t\n{}\tr\ty\tFP\t\n{}\tr\tz\tFP\t\n",
            fp["bad_one"], fp["maybe_bad"], fp["maybe_unsure"]
        ),
    );
}

#[test]
fn a_model_that_passes_the_gate_is_attached_by_a_revision_and_checks_ask_it_first() {
    let dir = project(&declared());
    label_table(dir.path());
    assert_eq!(failures(dir.path()), 1);

    let out = attach(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let reply = json(&out);
    assert_eq!(reply["decision"], DECISION);
    assert_eq!(reply["model"], MODEL);
    assert!(reply.get("refused").is_none());
    assert_eq!(reply["evaluation"]["recall"]["hits"], 3);
    assert_eq!(reply["evaluation"]["precision"]["of"], 3);
    assert_eq!(reply["evaluation"]["labelled"], 4);
    assert_eq!(reply["evaluation"]["crossFitted"], 0);

    let records = Records::read(dir.path()).unwrap();
    assert_eq!(records.revisions.len(), 1);
    let revision = &records.revisions[0];
    assert_eq!(reply["revision"], revision.id.as_str());
    let spec = &revision.spec;
    assert_eq!(spec.decision, DECISION);
    assert_eq!(spec.parent, None);
    assert_eq!(spec.change, Change::Minor);
    assert_eq!(spec.model.as_deref(), Some(MODEL));
    assert_eq!(spec.approval.id.as_deref(), Some("kim"));
    for field in ["labelled", "crossFitted", "trainingFile"] {
        assert_eq!(
            spec.evaluation[field], reply["evaluation"][field],
            "{field}"
        );
    }
    assert_eq!(spec.evaluation["precision"]["hits"], 3);
    assert_eq!(spec.evaluation["policy"]["precision"], 0.9);
    assert!(spec.meaning_version.is_some() && spec.check_revision.is_some());
    assert_eq!(records.attached()[DECISION], MODEL);

    // The check asks the attached model first: it fails `maybe_bad` too.
    assert_eq!(failures(dir.path()), 2);
    let listed = lighthouse(dir.path(), &["model", "list", "--format", "json"]);
    assert!(json(&listed).to_string().contains("attached"));
}

#[test]
fn a_model_that_is_already_attached_is_refused() {
    let dir = project(&declared());
    label_table(dir.path());
    assert_eq!(attach(dir.path(), &[]).status.code(), Some(0));

    let out = attach(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(1));
    let reasons = json(&out)["refused"].to_string();
    assert!(reasons.contains("already attached"), "{reasons}");
    assert_eq!(Records::read(dir.path()).unwrap().revisions.len(), 1);
}

#[test]
fn a_model_that_misses_the_policy_is_refused_and_nothing_is_written() {
    let dir = project(&declared());
    mislabelled(dir.path());

    let out = attach(dir.path(), &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let reply = json(&out);
    assert!(reply.get("revision").is_none());
    let reasons = reply["refused"].to_string();
    assert!(reasons.contains("misses the policy"), "{reasons}");
    assert!(Records::read(dir.path()).unwrap().revisions.is_empty());
    assert_eq!(failures(dir.path()), 1);

    let text = lighthouse(
        dir.path(),
        &[
            "model",
            "attach",
            DECISION,
            "--labels",
            "labels.tsv",
            "--model",
            MODEL,
        ],
    );
    let shown = String::from_utf8_lossy(&text.stdout);
    assert!(shown.starts_with("refused:"), "{shown}");
    assert!(shown.contains("precision:"), "{shown}");
}

#[test]
fn detaching_unbinds_the_model_by_a_revision_that_follows_the_attaching_one() {
    let dir = project(&declared());
    label_table(dir.path());
    let attached = json(&attach(dir.path(), &[]))["revision"]
        .as_str()
        .unwrap()
        .to_owned();

    let out = lighthouse(
        dir.path(),
        &[
            "model",
            "detach",
            DECISION,
            "--attributed-to",
            "person:lee",
            "--format",
            "json",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let reply = json(&out);
    assert_eq!(reply["model"], MODEL);

    let records = Records::read(dir.path()).unwrap();
    assert_eq!(records.revisions.len(), 2);
    let spec = &records.revisions[1].spec;
    assert_eq!(reply["revision"], records.revisions[1].id.as_str());
    assert_eq!(spec.parent.as_deref(), Some(attached.as_str()));
    assert_eq!(spec.change, Change::Minor);
    assert_eq!(spec.model, None);
    assert_eq!(spec.evaluation, serde_json::json!({ "detached": MODEL }));
    assert_eq!(spec.approval.id.as_deref(), Some("lee"));
    assert!(records.attached().is_empty());
    assert_eq!(failures(dir.path()), 1);

    // Attaching again follows the detaching revision.
    assert_eq!(attach(dir.path(), &[]).status.code(), Some(0));
    let records = Records::read(dir.path()).unwrap();
    assert_eq!(records.revisions.len(), 3);
    assert_eq!(
        records.revisions[2].spec.parent.as_deref(),
        Some(records.revisions[1].id.as_str())
    );
}

#[test]
fn detaching_with_nothing_attached_is_a_usage_error() {
    let dir = project(&declared());
    let out = detach(dir.path());
    assert_eq!(out.status.code(), Some(2));
    let message = String::from_utf8_lossy(&out.stderr);
    assert!(
        message.contains("no model is attached to `local/naming-ban`"),
        "{message}"
    );
    assert!(!dir.path().join(".lighthouse/decisions.jsonl").exists());
}

#[test]
fn eval_proposes_detaching_an_attached_model_that_misses_the_policy_and_only_then() {
    let dir = project(&declared());
    label_table(dir.path());
    assert_eq!(attach(dir.path(), &[]).status.code(), Some(0));
    let eval = |dir: &Path| -> Value {
        json(&lighthouse(
            dir,
            &[
                "decision",
                "eval",
                DECISION,
                "--labels",
                "labels.tsv",
                "--format",
                "json",
            ],
        ))
    };
    assert_eq!(eval(dir.path())["current"]["layer"]["model"], MODEL);
    assert!(eval(dir.path()).get("proposal").is_none());

    // The labels now say the model's failure was a false alarm.
    mislabelled(dir.path());
    let report = eval(dir.path());
    assert_eq!(report["proposal"]["model"], MODEL);
    assert!(
        report["proposal"]["reason"]
            .as_str()
            .unwrap()
            .contains("precision"),
        "{report}"
    );
    // A proposal changes nothing.
    assert_eq!(Records::read(dir.path()).unwrap().revisions.len(), 1);
}
