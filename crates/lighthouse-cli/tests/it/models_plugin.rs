//! Model plugins end to end: a fake plugin process that speaks
//! `lighthouse/classify` and `lighthouse/train`, bound to a project, then
//! evaluated and trained through the command line.

use std::{collections::BTreeMap, fs, path::Path};

use assert_cmd::Command;
use serde_json::Value;

use crate::{
    cache_fixtures::write,
    models::{count, project, results},
};

/// A model plugin that speaks the protocol: it answers `classify` by the id of
/// each subject and keeps what `train` was given in the directory it is told.
const PLUGIN: &str = r#"#!/usr/bin/env python3
import json, sys

def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    return json.loads(sys.stdin.buffer.read(length))

def send(id, result):
    body = json.dumps({"jsonrpc": "2.0", "id": id, "result": result}).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()

while True:
    message = read()
    if message is None or message.get("method") == "exit":
        break
    method, params, id = message.get("method"), message.get("params", {}), message.get("id")
    if method == "initialize":
        send(id, {"id": "fake-model", "version": "1", "protocolVersion": "0.1", "languages": [],
                  "models": {"object": "list", "data": [{
                      "id": "fake-model", "object": "model", "created": 0, "owned_by": "fake",
                      "capabilities": {"classification": {"supported": True},
                                       "fine_tuning": {"supported": True}},
                      "version": "1", "cost": 0, "deterministic": True, "abstains": True}]}})
    elif method == "lighthouse/classify":
        text = params["state"]["text"]
        p = 0.95 if "maybe_bad" in text else 0.05 if "maybe_good" in text else 0.5
        send(id, {"model": params["model"],
                  "answers": {q: {"type": "noul", "noul": p} for q in params["questions"]}})
    elif method == "lighthouse/train":
        with open(params["training_file"]) as f:
            lines = [json.loads(l) for l in f]
        with open(params["metadata"]["dir"] + "/subjects.json", "w") as f:
            json.dump(lines, f)
        with open(params["metadata"]["dir"] + "/metrics.json", "w") as f:
            json.dump({"n": len(lines)}, f)
        send(id, {"id": "ftjob-1", "object": "fine_tuning.job", "model": params["model"],
                  "status": "succeeded", "fine_tuned_model": "sha256:abc",
                  "result_files": [params["metadata"]["dir"] + "/metrics.json"]})
    elif method == "shutdown":
        send(id, None)
"#;

const RPC: &str = "[models]\nclassification = \"fake/m1\"\n[models.providers.fake]\ntype = \"rpc\"\nplugin = \"fake-model\"\noptions = { tag = 1 }\n";

fn with_plugin(dir: &Path) {
    write(dir, "fakeplugin/plugin.py", PLUGIN);
    std::process::Command::new("chmod")
        .arg("+x")
        .arg(dir.join("fakeplugin/plugin.py"))
        .status()
        .unwrap();
    write(
        dir,
        "fakeplugin/lighthouse-plugin.toml",
        "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Plugin\"\n\n[metadata]\nname = \"fake-model\"\n\n[spec]\nversion = \"0.1.0\"\n\n[spec.runtime]\ncommand = \"./plugin.py\"\n\n[spec.provides]\nmodels = [\"fake-model\"]\n",
    );
    let text = fs::read_to_string(dir.join("lighthouse.toml")).unwrap();
    let text = text.replace(
        "\"core\", \"local\"",
        "\"core\", \"local\", { id = \"fake-model\", path = \"fakeplugin\" }",
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
            "agent-json",
            "--detail",
            "full",
        ],
    );
    let mut found = BTreeMap::new();
    for line in String::from_utf8(out.stdout).unwrap().lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
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
    assert_eq!(m["labelled"], 3);
    assert_eq!(m["precision"]["hits"], 2);
    assert_eq!(m["precision"]["of"], 2);
    assert_eq!(m["recall"]["of"], 2);
    assert_eq!(m["abstention"]["hits"], 1);
    assert_eq!(report["current"]["model_decided"], 1);
    assert_eq!(report["policy"]["precision"], 0.9);
}

#[test]
fn learn_train_hands_the_labelled_subjects_to_the_model_and_prints_its_metrics() {
    let dir = project(RPC);
    with_plugin(dir.path());
    baseline(dir.path());
    let out = lighthouse(
        dir.path(),
        &[
            "learn",
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
    assert_eq!(trained["result"]["artifact"], "sha256:abc");
    assert_eq!(trained["result"]["metrics"]["n"], 5);
    assert_eq!(trained["unresolved"], 0);
    // Three baseline rows and two examples.
    assert_eq!(trained["subjects"], 5);
    assert_eq!(trained["by_source"]["baseline"], 3);
    assert_eq!(trained["by_source"]["example"], 2);
    let kept = fs::read_to_string(
        dir.path()
            .join(".lighthouse/models/fake/local-naming-ban/subjects.json"),
    )
    .unwrap();
    let subjects: Vec<Value> = serde_json::from_str(&kept).unwrap();
    assert!(subjects.iter().all(|s| s["weight"] == 1.0));
    assert!(
        subjects
            .iter()
            .any(|s| s["label"] == "pass" && s["source"] == "baseline")
    );
}
