//! Applying what a fix previewed: by fingerprint, as it was shown, once for a
//! batch, and declined when its base changed.

use std::{fs, path::Path};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{
    cache_fixtures::write,
    fix::{lighthouse, read},
    fx::{LONG, PROVIDERS, fix, project},
    mcp::Client,
};

/// The fingerprint of the one preview of `done`, and its apply command as
/// arguments of the binary.
fn previewed(done: &Value) -> (String, Vec<String>) {
    let preview = &done["previews"][0];
    let command = preview["apply"].as_str().unwrap();
    let args = command
        .split_whitespace()
        .skip(1)
        .map(str::to_owned)
        .collect();
    (preview["fingerprint"].as_str().unwrap().to_owned(), args)
}

fn calls(marks: &Path) -> usize {
    fs::read_to_string(marks).map_or(0, |text| text.lines().count())
}

/// The model fix project of `edited`, with an agent that counts its runs.
fn counted() -> (TempDir, TempDir, TempDir) {
    let marks = tempfile::tempdir().unwrap();
    let agent = format!(
        "printf 'x\\n' > model.txt\necho x >> {:?}\n",
        marks.path().join("calls").to_str().unwrap()
    );
    let (dir, scripts) = project("");
    write(scripts.path(), "agent.sh", &agent);
    (dir, scripts, marks)
}

#[test]
fn a_preview_is_applied_by_its_fingerprint_and_the_model_is_asked_once() {
    let (dir, _agent, marks) = counted();
    let done = fix(dir.path(), json!({"decision": ["local/model"]}));
    let (fingerprint, args) = previewed(&done);
    assert_eq!(calls(&marks.path().join("calls")), 1);

    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(&args)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("1 fix(es) applied"),
        "{out:?}"
    );

    assert_eq!(read(&dir, "model.txt"), "x\n", "{fingerprint}");
    assert_eq!(read(&dir, "ops-unsafe.txt"), LONG, "only that fix");
    assert_eq!(
        calls(&marks.path().join("calls")),
        1,
        "the model is asked once"
    );
}

#[test]
fn the_tool_call_of_a_preview_applies_exactly_that_fix() {
    let (dir, _agent, marks) = counted();
    let mut client = Client::trusting(dir.path());
    let done = client.tool(
        "fix",
        json!({"decision": ["local/model", "local/ops-unsafe"]}),
    );
    let done = done.unwrap();
    assert_eq!(done["previews"].as_array().unwrap().len(), 2, "{done}");
    let model = done["previews"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["decision"] == "local/model")
        .unwrap();

    let applied = client
        .tool("fix", model["tool"]["arguments"].clone())
        .unwrap();

    assert_eq!(applied["applied"].as_array().unwrap().len(), 1, "{applied}");
    assert_eq!(applied["applied"][0]["decision"], "local/model");
    assert_eq!(read(&dir, "model.txt"), "x\n");
    assert_eq!(read(&dir, "ops-unsafe.txt"), LONG);
    assert_eq!(calls(&marks.path().join("calls")), 1);
}

#[test]
fn a_deterministic_preview_is_applied_as_it_was_shown() {
    let (dir, _agent) = project("");
    let done = fix(dir.path(), json!({"decision": ["local/command-unsafe"]}));
    let (_, args) = previewed(&done);

    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(&args)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("1 fix(es) applied"),
        "{out:?}"
    );

    assert_eq!(read(&dir, "command-unsafe.txt"), "A\nB\nC\nD\n");
}

#[test]
fn a_changed_base_gives_a_conflict_and_nothing_is_written() {
    let (dir, _agent, marks) = counted();
    let done = fix(dir.path(), json!({"decision": ["local/model"]}));
    let (_, args) = previewed(&done);
    write(dir.path(), "model.txt", "a\nb\nc\nd\ne\n");

    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(&args)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not fixed model.txt"), "{stderr}");
    assert_eq!(read(&dir, "model.txt"), "a\nb\nc\nd\ne\n");
    assert_eq!(calls(&marks.path().join("calls")), 1, "no new model call");
}

#[test]
fn a_conflict_is_the_class_of_the_decline() {
    let (dir, _agent, _marks) = counted();
    let done = fix(dir.path(), json!({"decision": ["local/model"]}));
    let preview = done["previews"][0]["tool"]["arguments"].clone();
    write(dir.path(), "model.txt", "a\nb\nc\nd\ne\n");

    let applied = fix(dir.path(), preview);

    assert!(
        applied["applied"].as_array().unwrap().is_empty(),
        "{applied}"
    );
    assert_eq!(
        applied["models"]["local/model"]["declined"],
        json!({"conflict": 1}),
        "{applied}"
    );
}

#[test]
fn no_models_asks_no_model_and_leaves_the_finding_to_review() {
    let (dir, _agent, marks) = counted();

    let done = fix(
        dir.path(),
        json!({"decision": ["local/model"], "noModels": true}),
    );

    assert_eq!(calls(&marks.path().join("calls")), 0);
    assert_eq!(read(&dir, "model.txt"), LONG);
    assert!(done["applied"].as_array().unwrap().is_empty(), "{done}");
    let reason = done["declined"][0]["reason"].as_str().unwrap();
    assert!(reason.starts_with("left for review"), "{reason}");
    assert!(done["usage"].is_null(), "{done}");
}

#[test]
fn the_command_line_asks_no_model_with_the_flag_check_has() {
    let (dir, _agent, marks) = counted();

    lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(["check", "--fix", "--no-models", "--decision", "local/model"])
        .assert()
        .stderr(predicates::str::contains("left for review"));

    assert_eq!(calls(&marks.path().join("calls")), 0);
}

/// The project with a second decision over `model.txt`, fixed by the same
/// agent: two findings in one file, which one run of the agent fixes.
fn batched() -> (TempDir, TempDir, TempDir) {
    let (dir, scripts, marks) = counted();
    let first = read(&dir, ".lighthouse/decisions/model.yaml");
    write(
        dir.path(),
        ".lighthouse/decisions/model-two.yaml",
        &first
            .replace("name: local/model\n", "name: local/model-two\n")
            .replace("file.lines > 3", "file.lines > 2"),
    );
    let toml = read(&dir, "lighthouse.toml");
    write(
        dir.path(),
        "lighthouse.toml",
        &toml
            .replace(
                "\"decisions\" = [\"local/model\"]",
                "\"decisions\" = [\"local/model\", \"local/model-two\"]",
            )
            .replace(
                "\"rules\" = { ",
                "\"rules\" = { \"local/model-two\" = \"warn\", ",
            ),
    );
    (dir, scripts, marks)
}

#[test]
fn the_findings_of_one_run_of_an_agent_are_previewed_and_applied_together() {
    let (dir, _agent, marks) = batched();
    let done = fix(
        dir.path(),
        json!({"decision": ["local/model", "local/model-two"]}),
    );
    let previews = done["previews"].as_array().unwrap();
    assert_eq!(previews.len(), 2, "{done}");
    assert_eq!(calls(&marks.path().join("calls")), 1, "one run for both");
    let batch: Vec<&str> = previews
        .iter()
        .map(|p| p["fingerprint"].as_str().unwrap())
        .collect();
    for preview in previews {
        let fingerprints = preview["tool"]["arguments"]["fingerprints"]
            .as_array()
            .unwrap();
        assert_eq!(
            fingerprints.len(),
            2,
            "every apply names the batch: {preview}"
        );
        for member in &batch {
            assert!(
                fingerprints
                    .iter()
                    .any(|f| member.starts_with(f.as_str().unwrap())),
                "{member} {preview}"
            );
        }
    }
    let (_, args) = previewed(&done);

    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(&args)
        .output()
        .unwrap();

    assert!(
        String::from_utf8_lossy(&out.stderr).contains("2 fix(es) applied"),
        "{out:?}"
    );
    assert_eq!(read(&dir, "model.txt"), "x\n");
    assert_eq!(
        calls(&marks.path().join("calls")),
        1,
        "the agent is not asked again"
    );
}

#[test]
fn applying_a_preview_without_the_cache_declines_it_as_not_kept() {
    let (dir, _agent, marks) = counted();
    let done = fix(dir.path(), json!({"decision": ["local/model"]}));
    let (_, mut args) = previewed(&done);
    args.push("--no-cache".to_owned());

    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(&args)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("the preview is not kept (run without --no-cache, or fix again)"),
        "{stderr}"
    );
    assert_eq!(read(&dir, "model.txt"), LONG);
    assert_eq!(
        calls(&marks.path().join("calls")),
        1,
        "the agent is not asked again"
    );
}

#[test]
fn a_preview_that_was_never_made_is_not_made_by_applying_it() {
    for provider in PROVIDERS.iter().filter(|p| p.safety == "unsafe") {
        let (dir, _agent) = project("");
        let shown = fix(
            dir.path(),
            json!({"decision": [provider.id], "dryRun": true}),
        );
        let fingerprint = shown["previews"][0]["fingerprint"].as_str().unwrap();
        let (other, _other_agent) = project("");

        let applied = fix(
            other.path(),
            json!({"fingerprints": [fingerprint], "unsafeFixes": true}),
        );

        assert!(
            applied["applied"].as_array().unwrap().is_empty(),
            "{applied}"
        );
        assert_eq!(read(&other, provider.file), LONG, "{}", provider.id);
    }
}

#[test]
fn under_allow_a_fingerprint_names_what_to_fix_and_the_fix_is_written() {
    let (dir, _agent) = project("unsafe = true\n");
    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args([
            "check",
            "--format",
            "json",
            "--decision",
            "local/command-unsafe",
        ])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let fingerprint = report["groups"][0]["files"]["command-unsafe.txt"][0][2]
        .as_str()
        .unwrap()
        .to_owned();

    let done = fix(dir.path(), json!({"fingerprints": [fingerprint]}));

    assert_eq!(done["applied"].as_array().unwrap().len(), 1, "{done}");
    assert_eq!(read(&dir, "command-unsafe.txt"), "A\nB\nC\nD\n");
    assert!(done["previews"].is_null(), "{done}");
}

#[test]
fn a_deterministic_preview_over_a_changed_base_is_a_conflict_and_is_not_made_again() {
    let (dir, _agent) = project("");
    let done = fix(dir.path(), json!({"decision": ["local/command-unsafe"]}));
    let (_, args) = previewed(&done);
    write(dir.path(), "command-unsafe.txt", "a\nb\nc\nd\ne\n");

    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(&args)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("is not as it was when the preview was made"),
        "{stderr}"
    );
    assert_eq!(read(&dir, "command-unsafe.txt"), "a\nb\nc\nd\ne\n");
}
