//! Model plugins end to end: a fake plugin process that speaks
//! `lighthouse/classify` and `lighthouse/train`, bound to a project, then
//! evaluated and trained through the command line.

use std::{collections::BTreeMap, fs, path::Path};

use assert_cmd::Command;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    cache_fixtures::write,
    models::{count, project, results},
};

const RPC: &str = "[models]\nclassification = \"fake/m1\"\n[models.providers.fake]\ntype = \"rpc\"\nplugin = \"fake-model\"\noptions = { tag = 1 }\n";

/// Binds the `fake-model` plugin, which speaks `lighthouse/classify` and
/// `lighthouse/train`.
fn with_plugin(dir: &Path) {
    let plugin = lighthouse_test_support::fake_model();
    let text = fs::read_to_string(dir.join("lighthouse.toml")).unwrap();
    let text = text.replace(
        "\"core\", \"local\"",
        &format!(
            "\"core\", \"local\", {{ id = \"fake-model\", path = {:?} }}",
            plugin.to_str().unwrap()
        ),
    );
    fs::write(dir.join("lighthouse.toml"), text).unwrap();
}

fn lighthouse(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(dir)
        .env("LIGHTHOUSE_TRUST", "1")
        .args(args)
        .output()
        .unwrap()
}

/// Fingerprints of the subjects of the decision, by function name.
fn fingerprints(dir: &Path) -> BTreeMap<String, String> {
    let out = lighthouse(
        dir,
        &[
            "check",
            "--no-store",
            "--no-cache",
            "--no-models",
            "--format",
            "json",
            "--detail",
            "full",
        ],
    );
    let mut found = BTreeMap::new();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    for v in report["findings"].as_array().unwrap() {
        if v["rule"] == "local/naming-ban" {
            let message = v["message"].as_str().unwrap();
            let name = message.split(' ').next().unwrap().to_owned();
            found.insert(name, v["fingerprint"].as_str().unwrap().to_owned());
        }
    }
    found
}

/// A table that calls the two flagged functions right and the unsure one wrong.
fn baseline(dir: &Path) {
    let fp = fingerprints(dir);
    assert_eq!(fp.len(), 4, "{fp:?}");
    write(
        dir,
        "labels.tsv",
        &format!(
            "fingerprint\trepo\tlocation\tverdict\treason\n{}\tr\tx\tTP\t\n{}\tr\ty\tTP\t\n{}\tr\tz\tFP\t\n",
            fp["bad_one"], fp["maybe_bad"], fp["maybe_unsure"]
        ),
    );
}

fn json(out: &std::process::Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn a_model_plugin_bound_to_the_project_decides_the_abstentions() {
    let dir = project(RPC);
    with_plugin(dir.path());
    let out = lighthouse(
        dir.path(),
        &["check", "--no-store", "--no-cache", "--format", "sarif"],
    );
    let found = results(&json(&out));
    assert_eq!(count(&found, "fail"), 2, "{found:?}");
    assert_eq!(count(&found, "review"), 2);
}

#[test]
fn eval_measures_the_check_with_a_model_plugin_bound() {
    let dir = project(RPC);
    with_plugin(dir.path());
    baseline(dir.path());
    let out = lighthouse(
        dir.path(),
        &[
            "decision",
            "eval",
            "local/naming-ban",
            "--baseline",
            "labels.tsv",
            "--format",
            "json",
        ],
    );
    let report = json(&out);
    let m = &report["current"]["metrics"];
    // Three reviewed rows and the one subject the invalid example shows.
    assert_eq!(m["labelled"], 4);
    assert_eq!(m["precision"]["hits"], 3);
    assert_eq!(m["precision"]["of"], 3);
    assert_eq!(m["recall"]["of"], 3);
    assert_eq!(m["abstention"]["hits"], 1);
    assert_eq!(report["current"]["model_decided"], 1);
    assert_eq!(report["policy"]["precision"], 0.9);
    assert_eq!(report["meets_policy"], true);
}

#[test]
fn learn_train_hands_the_labelled_subjects_to_the_model_and_prints_its_metrics() {
    let dir = project(RPC);
    with_plugin(dir.path());
    baseline(dir.path());
    let out = lighthouse(
        dir.path(),
        &[
            "model",
            "train",
            "local/naming-ban",
            "--baseline",
            "labels.tsv",
            "--format",
            "json",
        ],
    );
    let trained = json(&out);
    assert_eq!(trained["model"], "fake/m1");
    let digest = trained["training_file"].as_str().unwrap();
    assert!(digest.starts_with("sha256:"), "{digest}");
    assert_eq!(trained["result"]["status"], "succeeded");
    assert_eq!(trained["result"]["fine_tuned_model"], "sha256:abc");
    assert_eq!(trained["unresolved"], 0);
    // Three reviewed rows and one example.
    assert_eq!(trained["subjects"], 4);
    assert_eq!(trained["by_source"]["tsv"], 3);
    assert_eq!(trained["by_source"]["example"], 1);

    // The dataset is a file of the host's store named by its digest, in
    // canonical JSON, one training line per subject.
    let hex = digest.strip_prefix("sha256:").unwrap();
    let path = dir
        .path()
        .join(format!(".lighthouse/datasets/sha256/{hex}.jsonl"));
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(text.as_bytes())), hex);
    assert_eq!(text.lines().count(), 4);
    for line in text.lines() {
        let parsed: Value = serde_json::from_str(line).unwrap();
        assert_eq!(parsed["messages"].as_array().unwrap().len(), 3);
        assert!(parsed["weight"].is_number());
        assert_eq!(line, lighthouse_model::canonical(&parsed));
    }

    // The model was handed the same lines.
    let kept = fs::read_to_string(
        dir.path()
            .join(".lighthouse/models/fake/local-naming-ban/subjects.json"),
    )
    .unwrap();
    let subjects: Vec<Value> = serde_json::from_str(&kept).unwrap();
    assert_eq!(subjects.len(), 4);
    assert!(subjects.iter().any(|s| s["weight"] == 1.0));
}
