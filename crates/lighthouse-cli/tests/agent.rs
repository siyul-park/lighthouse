//! The agent integration through the binary: Claude Code hooks fed sample
//! payloads, and `init --agent claude-code` merging into existing files.

use std::{fs, path::Path, process::Command as Git};

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;

const BIG: &str = "a\nb\nc\nd\ne\nf\n";
const HELPER: &str = "pub fn run(x: u8) -> u8 {\n    clamp(x) + 1\n}\n\nfn clamp(x: u8) -> u8 {\n    if x > 10 { 10 } else { x }\n}\n";

fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir);
    cmd
}

/// A text project that fails files over three lines.
fn text_project() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        "plugins = [\"core\"]\n[rules]\n\"core/max-file-lines\" = { level = \"error\", max = 3 }\n",
    )
    .unwrap();
    fs::write(dir.path().join("big.txt"), BIG).unwrap();
    fs::write(dir.path().join("small.txt"), "a\n").unwrap();
    fs::write(dir.path().join("other.txt"), BIG).unwrap();
    dir
}

fn rust_project(extra: &str, source: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let plugin = lighthouse_testkit::lang_rust();
    fs::write(
        dir.path().join("lighthouse.toml"),
        format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"design\"]\nextends = [\"design/recommended\", \"design/strict\"]\n{extra}",
            plugin.to_str().unwrap()
        ),
    )
    .unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), source).unwrap();
    dir
}

fn edit(dir: &Path, file: &str) -> Value {
    json!({
        "session_id": "s1",
        "cwd": dir.to_str().unwrap(),
        "hook_event_name": "PostToolUse",
        "tool_name": "Edit",
        "tool_input": { "file_path": dir.join(file).to_str().unwrap() },
        "tool_response": "ok",
    })
}

fn stop(dir: &Path, active: bool) -> Value {
    json!({
        "session_id": "s1",
        "cwd": dir.to_str().unwrap(),
        "hook_event_name": "Stop",
        "stop_hook_active": active,
    })
}

/// Runs the hook; returns its exit code and the JSON it printed, if any.
fn hook(dir: &Path, args: &[&str], payload: &Value) -> (i32, Option<Value>) {
    let out = lighthouse(dir)
        .args(["hook", "claude-code"])
        .args(args)
        .write_stdin(payload.to_string())
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let json = (!text.trim().is_empty()).then(|| serde_json::from_str(&text).unwrap());
    (out.status.code().unwrap(), json)
}

fn git(dir: &Path, args: &[&str]) {
    let status = Git::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap()
        .status;
    assert!(status.success(), "git {args:?}");
}

#[test]
fn an_edit_with_errors_is_fed_back_as_a_block() {
    let dir = text_project();
    let (code, reply) = hook(dir.path(), &["post-tool-use"], &edit(dir.path(), "big.txt"));
    assert_eq!(code, 0);
    let reply = reply.unwrap();
    assert_eq!(reply["decision"], "block");
    let reason = reply["reason"].as_str().unwrap();
    assert!(reason.contains("core/max-file-lines"), "{reason}");
    assert!(reason.contains("big.txt"), "{reason}");
    assert!(
        !reason.contains("other.txt"),
        "only the edited file is reported"
    );
}

#[test]
fn a_clean_edit_and_files_not_worth_checking_are_silent() {
    let dir = text_project();
    let silent =
        |payload: Value| assert_eq!(hook(dir.path(), &["post-tool-use"], &payload), (0, None));
    silent(edit(dir.path(), "small.txt"));
    silent(edit(dir.path(), "missing.txt"));
    let mut read = edit(dir.path(), "big.txt");
    read["tool_name"] = json!("Read");
    silent(read);
    fs::write(dir.path().join("image.bin"), [0u8, 1, 2, 3]).unwrap();
    silent(edit(dir.path(), "image.bin"));
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::write(dir.path().join(".git/big.txt"), BIG).unwrap();
    silent(edit(dir.path(), ".git/big.txt"));
    let mut outside = edit(dir.path(), "big.txt");
    outside["tool_input"]["file_path"] = json!("/etc/hosts");
    silent(outside);
}

#[test]
fn a_directory_without_lighthouse_is_left_alone_and_a_bad_payload_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        hook(dir.path(), &["post-tool-use"], &edit(dir.path(), "x.txt")),
        (0, None)
    );
    let out = lighthouse(dir.path())
        .args(["hook", "claude-code", "stop"])
        .write_stdin("not json")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "1 is non-blocking, 2 would block"
    );
}

#[test]
fn review_findings_are_context_not_a_block() {
    let dir = rust_project("[rules]\n\"design/exported-doc\" = \"off\"\n", HELPER);
    let (code, reply) = hook(
        dir.path(),
        &["post-tool-use"],
        &edit(dir.path(), "src/lib.rs"),
    );
    assert_eq!(code, 0);
    let reply = reply.unwrap();
    assert!(reply.get("decision").is_none(), "{reply}");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    assert!(context.contains("1 finding(s)"), "{context}");
    assert!(context.contains("review_resolve"), "{context}");
}

#[test]
fn incomplete_analysis_is_stated_and_blocks_only_without_allow_incomplete() {
    let dir = rust_project("", "fn (((\n");
    let payload = edit(dir.path(), "src/lib.rs");
    let (_, strict) = hook(dir.path(), &["post-tool-use"], &payload);
    assert_eq!(strict.unwrap()["decision"], "block");
    let (code, lenient) = hook(
        dir.path(),
        &["post-tool-use", "--allow-incomplete"],
        &payload,
    );
    assert_eq!(code, 0);
    let lenient = lenient.unwrap();
    assert!(lenient.get("decision").is_none(), "{lenient}");
    assert!(
        lenient["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("INCOMPLETE")
    );
}

#[test]
fn stop_blocks_on_remaining_errors_once() {
    let dir = text_project();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &["add", "small.txt", "other.txt", "lighthouse.toml"],
    );
    git(dir.path(), &["commit", "-q", "-m", "init"]);

    let (code, reply) = hook(dir.path(), &["stop"], &stop(dir.path(), false));
    assert_eq!(code, 0);
    let reply = reply.unwrap();
    assert_eq!(reply["decision"], "block");
    assert!(reply["reason"].as_str().unwrap().contains("big.txt"));

    let (_, forced) = hook(dir.path(), &["stop"], &stop(dir.path(), true));
    let forced = forced.unwrap();
    assert!(forced.get("decision").is_none(), "no loop: {forced}");
    assert!(forced["systemMessage"].as_str().unwrap().contains("remain"));

    fs::remove_file(dir.path().join("big.txt")).unwrap();
    assert_eq!(
        hook(dir.path(), &["stop"], &stop(dir.path(), false)),
        (0, None)
    );
}

#[test]
fn hooks_report_fixable_findings_but_never_fix_them() {
    let source = "pub fn run() -> u8 {\n    1\n}\n\npub struct Store;\n";
    let dir = rust_project("", source);

    let (code, reply) = hook(
        dir.path(),
        &["post-tool-use"],
        &edit(dir.path(), "src/lib.rs"),
    );
    assert_eq!(code, 0);
    let reply = reply.expect("the error is fed back");
    assert!(reply.to_string().contains("declaration-groups"), "{reply}");
    let (_, forced) = hook(dir.path(), &["stop"], &stop(dir.path(), false));
    assert!(forced.is_some());

    assert_eq!(
        fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        source,
        "a hook never changes a file"
    );
}

#[test]
fn init_for_claude_code_merges_without_overwriting_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join(".mcp.json"),
        r#"{"mcpServers":{"other":{"command":"x"}},"keep":1}"#,
    )
    .unwrap();
    fs::create_dir(dir.path().join(".claude")).unwrap();
    fs::write(
        dir.path().join(".claude/settings.json"),
        r#"{"permissions":{"allow":["Bash(ls)"]},"hooks":{"PostToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"mine"}]}]}}"#,
    )
    .unwrap();

    let out = lighthouse(dir.path())
        .args(["init", "--agent", "claude-code"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let printed = String::from_utf8(out.stdout).unwrap();
    assert!(
        printed.contains("added the `lighthouse` MCP server"),
        "{printed}"
    );

    let read = |p: &str| -> Value {
        serde_json::from_str(&fs::read_to_string(dir.path().join(p)).unwrap()).unwrap()
    };
    let mcp = read(".mcp.json");
    assert_eq!(mcp["keep"], 1);
    assert_eq!(mcp["mcpServers"]["other"]["command"], "x");
    assert_eq!(mcp["mcpServers"]["lighthouse"]["args"], json!(["mcp"]));
    let settings = read(".claude/settings.json");
    assert_eq!(settings["permissions"]["allow"][0], "Bash(ls)");
    assert_eq!(
        settings["permissions"]["deny"],
        json!(["Bash(lighthouse trust:*)"]),
        "an agent may not trust a project for the user"
    );
    let post = settings["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 2);
    assert_eq!(post[0]["hooks"][0]["command"], "mine");
    assert_eq!(post[1]["matcher"], "Edit|Write|MultiEdit");
    assert!(
        post[1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .starts_with("lighthouse hook claude-code post-tool-use")
    );
    assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 1);
    assert_eq!(post[1]["hooks"][0]["timeout"], 120);
    assert_eq!(settings["hooks"]["Stop"][0]["hooks"][0]["timeout"], 300);
    let skill = fs::read_to_string(dir.path().join(".claude/skills/lighthouse/SKILL.md")).unwrap();
    assert!(skill.starts_with("---\nname: lighthouse"));
    assert!(dir.path().join("lighthouse.toml").is_file());

    let snapshot = |p: &str| fs::read_to_string(dir.path().join(p)).unwrap();
    let before = (
        snapshot(".mcp.json"),
        snapshot(".claude/settings.json"),
        skill,
    );
    let again = lighthouse(dir.path())
        .args(["init", "--agent", "claude-code"])
        .output()
        .unwrap();
    assert!(again.status.success());
    assert!(
        String::from_utf8(again.stdout)
            .unwrap()
            .contains("unchanged")
    );
    let after = (
        snapshot(".mcp.json"),
        snapshot(".claude/settings.json"),
        snapshot(".claude/skills/lighthouse/SKILL.md"),
    );
    assert_eq!(before, after);

    // A skill file the user wrote is theirs.
    fs::write(
        dir.path().join(".claude/skills/lighthouse/SKILL.md"),
        "mine\n",
    )
    .unwrap();
    lighthouse(dir.path())
        .args(["init", "--agent", "claude-code"])
        .assert()
        .success();
    assert_eq!(snapshot(".claude/skills/lighthouse/SKILL.md"), "mine\n");
}

#[test]
fn init_without_the_flag_is_unchanged_and_a_broken_settings_file_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    lighthouse(dir.path()).arg("init").assert().success();
    assert!(!dir.path().join(".mcp.json").exists());
    assert!(!dir.path().join(".claude").exists());
    lighthouse(dir.path()).arg("init").assert().code(2);

    fs::create_dir(dir.path().join(".claude")).unwrap();
    fs::write(dir.path().join(".claude/settings.json"), "{ nope").unwrap();
    lighthouse(dir.path())
        .args(["init", "--agent", "claude-code"])
        .assert()
        .code(2);
    assert_eq!(
        fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
        "{ nope"
    );
}

#[test]
fn docs_generate_writes_the_skill_and_check_covers_it() {
    let dir = tempfile::tempdir().unwrap();
    lighthouse(dir.path())
        .args(["docs", "check"])
        .assert()
        .code(1);
    lighthouse(dir.path())
        .args(["docs", "generate"])
        .assert()
        .success();
    let skill = dir.path().join("skills/lighthouse/SKILL.md");
    let text = fs::read_to_string(&skill).unwrap();
    assert!(text.contains("## Active patterns"));
    lighthouse(dir.path())
        .args(["docs", "check"])
        .assert()
        .success();
    fs::write(&skill, format!("{text}edit\n")).unwrap();
    lighthouse(dir.path())
        .args(["docs", "check"])
        .assert()
        .code(1);
}

#[test]
fn a_broken_config_is_told_to_the_agent_not_swallowed() {
    let dir = text_project();
    fs::write(dir.path().join("lighthouse.toml"), "plugins = [").unwrap();
    let (code, reply) = hook(dir.path(), &["post-tool-use"], &edit(dir.path(), "big.txt"));
    assert_eq!(code, 0);
    let reply = reply.unwrap();
    assert!(reply.get("decision").is_none(), "{reply}");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("nothing was checked"), "{context}");

    let (_, blocked) = hook(dir.path(), &["stop"], &stop(dir.path(), false));
    let blocked = blocked.unwrap();
    assert_eq!(blocked["decision"], "block");
    assert!(
        blocked["reason"]
            .as_str()
            .unwrap()
            .contains("nothing was checked")
    );
    let (_, forced) = hook(dir.path(), &["stop"], &stop(dir.path(), true));
    assert!(forced.unwrap().get("decision").is_none());
}

#[test]
fn stop_without_a_commit_or_a_repository_still_reports() {
    let fresh = text_project();
    git(fresh.path(), &["init", "-q"]);
    let (_, reply) = hook(fresh.path(), &["stop"], &stop(fresh.path(), false));
    let reply = reply.unwrap();
    assert_eq!(
        reply["decision"], "block",
        "no commit yet: everything is changed"
    );
    assert!(reply["reason"].as_str().unwrap().contains("big.txt"));

    let plain = text_project();
    let (_, reply) = hook(plain.path(), &["stop"], &stop(plain.path(), false));
    let reason = reply.unwrap()["reason"].as_str().unwrap().to_owned();
    assert!(reason.contains("whole project was reported"), "{reason}");
    assert!(reason.contains("big.txt"), "{reason}");
}

#[test]
fn stop_tells_the_agent_about_gaps_once_even_with_allow_incomplete() {
    let dir = rust_project("", "fn (((\n");
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "-m", "init"]);
    let (_, first) = hook(
        dir.path(),
        &["stop", "--allow-incomplete"],
        &stop(dir.path(), false),
    );
    let first = first.unwrap();
    assert_eq!(first["decision"], "block");
    assert!(first["reason"].as_str().unwrap().contains("INCOMPLETE"));
    let (_, second) = hook(
        dir.path(),
        &["stop", "--allow-incomplete"],
        &stop(dir.path(), true),
    );
    let second = second.unwrap();
    assert!(second.get("decision").is_none());
    assert!(
        second["systemMessage"]
            .as_str()
            .unwrap()
            .contains("incomplete")
    );
}
