//! The hooks report what the session introduced: PreToolUse records a file's
//! findings per Claude Code session, PostToolUse and Stop keep the new and the
//! updated ones against it.

use std::{fs, path::Path};

use serde_json::{Value, json};

use super::agent::{BIG, edit, git, hook, lighthouse, pre, pre_payload, stop, text_project};

const HOOKS_DIR: &str = ".lighthouse/cache/hooks";

fn post(dir: &Path, file: &str) -> (i32, Option<Value>) {
    hook(dir, &["post-tool-use"], &edit(dir, file))
}

fn reason(reply: Option<Value>) -> String {
    let reply = reply.expect("a reply");
    assert_eq!(reply["decision"], "block", "{reply}");
    reply["reason"].as_str().unwrap().to_owned()
}

#[test]
fn stop_without_a_baseline_is_silent_in_a_dirty_tree() {
    let dir = text_project();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["add", "small.txt", "lighthouse.toml"]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    assert_eq!(
        hook(dir.path(), &["stop"], &stop(dir.path(), false)),
        (0, None)
    );
}

#[test]
fn an_edit_that_leaves_an_existing_finding_alone_is_silent() {
    let dir = text_project();
    let toml = fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        toml.replace("level = \"error\"", "level = \"warn\""),
    )
    .unwrap();
    pre(dir.path(), "big.txt");
    fs::write(dir.path().join("big.txt"), "z\nb\nc\nd\ne\nf\n").unwrap();
    assert_eq!(post(dir.path(), "big.txt"), (0, None));
    assert_eq!(
        hook(dir.path(), &["stop"], &stop(dir.path(), false)),
        (0, None)
    );
}

#[test]
fn an_edit_that_introduces_an_error_blocks_with_that_finding_only() {
    let dir = text_project();
    pre(dir.path(), "small.txt");
    fs::write(dir.path().join("small.txt"), BIG).unwrap();
    let reason = reason(post(dir.path(), "small.txt").1);
    assert!(reason.contains("core/max-lines"), "{reason}");
    assert!(reason.contains("small.txt"), "{reason}");
    assert!(!reason.contains("no pre-edit baseline"), "{reason}");
}

#[test]
fn stop_names_only_the_error_the_session_introduced() {
    let dir = text_project();
    pre(dir.path(), "small.txt");
    fs::write(dir.path().join("small.txt"), BIG).unwrap();
    let reason = reason(hook(dir.path(), &["stop"], &stop(dir.path(), false)).1);
    assert!(reason.contains("small.txt"), "{reason}");
    assert!(reason.contains("introduced this session"), "{reason}");
    for untouched in ["other.txt", "big.txt"] {
        assert!(!reason.contains(untouched), "{untouched}: {reason}");
    }
}

#[test]
fn an_edit_that_changes_the_evidence_of_a_finding_is_reported_as_updated() {
    let dir = text_project();
    pre(dir.path(), "big.txt");
    fs::write(dir.path().join("big.txt"), format!("{BIG}g\nh\n")).unwrap();
    let reason = reason(post(dir.path(), "big.txt").1);
    assert!(reason.contains("8 lines"), "{reason}");
}

#[test]
fn an_edit_with_no_baseline_shows_the_whole_file_and_says_so() {
    let dir = text_project();
    let reason = reason(post(dir.path(), "big.txt").1);
    assert!(reason.contains("big.txt"), "{reason}");
    assert!(reason.contains("no pre-edit baseline"), "{reason}");
}

#[test]
fn a_session_id_that_is_not_a_name_is_no_session_and_writes_nothing() {
    let dir = text_project();
    let mut payload = pre_payload(dir.path(), "small.txt");
    payload["session_id"] = json!("../x");
    assert_eq!(hook(dir.path(), &["pre-tool-use"], &payload), (0, None));
    assert!(!dir.path().join(HOOKS_DIR).exists());
    assert!(!dir.path().join(".lighthouse/cache/x").exists());
    assert!(!dir.path().join(".lighthouse/x").exists());

    let mut stopping = stop(dir.path(), false);
    stopping["session_id"] = json!("../x");
    assert_eq!(hook(dir.path(), &["stop"], &stopping), (0, None));
    let mut edited = edit(dir.path(), "big.txt");
    edited["session_id"] = json!("../x");
    let (_, reply) = hook(dir.path(), &["post-tool-use"], &edited);
    assert!(reason(reply).contains("no pre-edit baseline"));
}

#[test]
fn a_file_the_session_creates_has_only_new_findings() {
    let dir = text_project();
    pre(dir.path(), "made.txt");
    let baselines = fs::read_dir(dir.path().join(HOOKS_DIR).join("s1"))
        .unwrap()
        .count();
    assert_eq!(baselines, 1, "an empty baseline, with no check");
    fs::write(dir.path().join("made.txt"), BIG).unwrap();
    let mut payload = edit(dir.path(), "made.txt");
    payload["tool_name"] = json!("Write");
    let reason = reason(hook(dir.path(), &["post-tool-use"], &payload).1);
    assert!(reason.contains("made.txt"), "{reason}");
    assert!(!reason.contains("no pre-edit baseline"), "{reason}");
    assert!(
        hook(dir.path(), &["stop"], &stop(dir.path(), false))
            .1
            .is_some()
    );
}

#[test]
fn a_second_touch_keeps_the_baseline_from_before_the_first_edit() {
    let dir = text_project();
    pre(dir.path(), "small.txt");
    fs::write(dir.path().join("small.txt"), BIG).unwrap();
    pre(dir.path(), "small.txt");
    assert!(post(dir.path(), "small.txt").1.is_some());
}

#[test]
fn init_adds_the_pre_tool_use_group_once_and_knows_a_hand_written_one() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".claude")).unwrap();
    let mine = r#"{"hooks":{"PreToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"lighthouse hook claude-code pre-tool-use"}]}]}}"#;
    fs::write(dir.path().join(".claude/settings.json"), mine).unwrap();
    lighthouse(dir.path())
        .args(["init", "--agent", "claude-code"])
        .assert()
        .success();
    let settings: Value = serde_json::from_str(
        &fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "Edit");
}
