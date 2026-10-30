//! The hooks record what a session says as signals, in the local store and
//! never in git; with surfacing on they tell the agent; and a decision an
//! agent creates is proposed, checked for duplicates and tied to its signals.

use std::{fs, path::Path, thread, time::Duration};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::agent::{BIG, hook, lighthouse, pre, stop, text_project};

const DIRECTIVE: &str = "From now on, never use unwrap() in library code.";

/// The planted key is assembled so that no scanner takes this file for a leak.
fn fake_key() -> String {
    format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8")
}

fn project(signals: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project(&format!(
            "plugins = []\n[rules]\n\"core/max-lines\" = {{ level = \"error\", options = {{ max = 3 }} }}\n[signals]\n{signals}\n"
        )),
    )
    .unwrap();
    fs::write(dir.path().join("big.txt"), BIG).unwrap();
    fs::write(dir.path().join("small.txt"), "a\n").unwrap();
    dir
}

fn prompt_payload(dir: &Path, session: &str, prompt: &str) -> Value {
    json!({
        "session_id": session,
        "cwd": dir.to_str().unwrap(),
        "hook_event_name": "UserPromptSubmit",
        "prompt": prompt,
    })
}

fn say(dir: &Path, prompt: &str) -> (i32, Option<Value>) {
    hook(
        dir,
        &["user-prompt-submit"],
        &prompt_payload(dir, "s1", prompt),
    )
}

fn start(dir: &Path) -> (i32, Option<Value>) {
    let payload = json!({
        "session_id": "s1",
        "cwd": dir.to_str().unwrap(),
        "hook_event_name": "SessionStart",
        "source": "startup",
    });
    hook(dir, &["session-start"], &payload)
}

/// `decision signals --all --format json`.
fn signals(dir: &Path) -> Value {
    let out = lighthouse(dir)
        .args(["decision", "signals", "--all", "--format", "json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

fn context(reply: Option<Value>, event: &str) -> String {
    let reply = reply.expect("a reply");
    assert_eq!(
        reply["hookSpecificOutput"]["hookEventName"], event,
        "{reply}"
    );
    reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn shadow_records_every_prompt_and_prints_nothing_to_the_agent() {
    let dir = project("");
    for text in [
        DIRECTIVE,
        "thanks",
        "Add a --verbose flag to the check command.",
    ] {
        assert_eq!(say(dir.path(), text), (0, None), "{text}");
    }
    assert_eq!(start(dir.path()), (0, None));
    assert_eq!(
        hook(dir.path(), &["stop"], &stop(dir.path(), false)),
        (0, None)
    );

    let all = signals(dir.path());
    assert_eq!(all["counts"]["total"], 3);
    assert_eq!(
        all["counts"]["byClass"],
        json!({ "directive": 1, "none": 1, "abstain": 1 })
    );
    assert_eq!(all["counts"]["abstained"], 1);
    assert_eq!(all["counts"]["byRule"]["en-always-never"], 1);
    assert_eq!(all["counts"]["byRule"]["en-never-mind"], 1);
    let rows = all["signals"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .all(|s| s["status"] == "open" && s["by"]["type"] == "Person")
    );
    let abstained = rows.iter().find(|s| s["class"] == "abstain").unwrap();
    assert_eq!(
        abstained["excerpt"],
        "Add a --verbose flag to the check command."
    );

    // By default only the open signals that state a decision are listed.
    let out = lighthouse(dir.path())
        .args(["decision", "signals", "--format", "json"])
        .output()
        .unwrap();
    let listed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listed["signals"].as_array().unwrap().len(), 1);
    assert_eq!(listed["signals"][0]["class"], "directive");
    assert_eq!(listed["counts"]["total"], 3);
}

#[test]
fn a_paste_is_not_the_persons_words() {
    let dir = project("");
    let pasted = "Summarize this:\n<pasted_content id=\"1\">\nYou must never log secrets.\n</pasted_content id=\"1\">";
    let unclosed = "Look <pasted_content id=\"2\"> you must never log secrets";
    let collapsed = "Look at [Pasted text #3 +20 lines]";
    for text in [pasted, unclosed, collapsed] {
        assert_eq!(say(dir.path(), text), (0, None), "{text}");
    }
    let all = signals(dir.path());
    assert_eq!(all["counts"]["byClass"], json!({ "abstain": 3 }));
    let rows = all["signals"].as_array().unwrap();
    assert!(rows.iter().all(|s| s["features"]["hadPasted"] == true));
    assert!(
        rows.iter()
            .all(|s| !s["excerpt"].as_str().unwrap().contains("secrets"))
    );
}

#[test]
fn empty_and_unknown_payloads_are_silent_and_a_payload_that_is_not_json_fails_softly() {
    let dir = project("surface = \"on\"");
    for event in [
        "pre-tool-use",
        "post-tool-use",
        "stop",
        "user-prompt-submit",
        "session-start",
    ] {
        // Without a `cwd` the project is the one the hook runs in.
        let (code, reply) = hook(dir.path(), &[event], &json!({}));
        assert_eq!(code, 0, "{event}");
        assert!(
            reply.is_none() || event == "session-start",
            "{event}: {reply:?}"
        );
        let payload =
            json!({ "cwd": dir.path().to_str().unwrap(), "unknown": [1, 2], "prompt": 7 });
        let (code, reply) = hook(dir.path(), &[event], &payload);
        assert_eq!(code, 0, "{event}");
        assert!(
            reply.is_none() || event == "session-start",
            "{event}: {reply:?}"
        );
    }
    let out = lighthouse(dir.path())
        .args(["hook", "claude-code", "user-prompt-submit"])
        .write_stdin("not json")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert_eq!(signals(dir.path())["counts"]["total"], 0);
}

#[test]
fn a_directory_without_a_project_gets_silence_and_no_store() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(say(dir.path(), DIRECTIVE), (0, None));
    assert_eq!(start(dir.path()), (0, None));
    assert!(!dir.path().join(".lighthouse").exists());
}

#[test]
fn a_planted_key_never_reaches_the_store() {
    let dir = project("");
    let key = fake_key();
    assert_eq!(
        say(
            dir.path(),
            &format!("Never paste {key} into a config file.")
        ),
        (0, None)
    );
    let all = signals(dir.path());
    let excerpt = all["signals"][0]["excerpt"].as_str().unwrap();
    assert!(excerpt.contains("[REDACTED]"), "{excerpt}");
    assert!(!all.to_string().contains(&key));
    let mut files = vec![dir.path().join(".lighthouse")];
    let mut seen = 0;
    while let Some(path) = files.pop() {
        if path.is_dir() {
            files.extend(fs::read_dir(&path).unwrap().flatten().map(|e| e.path()));
        } else {
            seen += 1;
            let bytes = fs::read(&path).unwrap();
            assert!(
                !bytes.windows(key.len()).any(|w| w == key.as_bytes()),
                "{} holds the key",
                path.display()
            );
        }
    }
    assert!(seen > 0, "the store was written");
}

#[test]
fn signals_older_than_the_retention_are_purged_when_a_new_one_is_written() {
    let dir = project("retention = \"1s\"");
    say(dir.path(), DIRECTIVE);
    assert_eq!(signals(dir.path())["counts"]["total"], 1);
    thread::sleep(Duration::from_millis(1500));
    say(dir.path(), "Always run cargo fmt before you commit.");
    let all = signals(dir.path());
    assert_eq!(all["counts"]["total"], 1);
    assert!(
        all["signals"][0]["excerpt"]
            .as_str()
            .unwrap()
            .contains("cargo fmt"),
        "{all}"
    );
}

#[test]
fn with_surfacing_on_the_agent_hears_of_a_decision_the_digest_and_the_open_signals_once() {
    let dir = project("surface = \"on\"");
    let said = context(say(dir.path(), DIRECTIVE).1, "UserPromptSubmit");
    assert!(said.contains("reads like a design decision"), "{said}");
    let id = signals(dir.path())["signals"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(said.contains(&format!("signal:{id}")), "{said}");
    assert!(said.contains("decision_create"), "{said}");
    // A message that is no decision gets nothing.
    assert_eq!(
        say(dir.path(), "Add a --verbose flag to the check command."),
        (0, None)
    );

    let digest = context(start(dir.path()).1, "SessionStart");
    assert!(digest.contains("core/max-lines"), "{digest}");
    assert!(digest.contains("ranked by"), "{digest}");

    let reason = context(
        hook(dir.path(), &["stop"], &stop(dir.path(), false)).1,
        "Stop",
    );
    assert!(
        reason.contains(&id) && reason.contains("1 signal(s)"),
        "{reason}"
    );
    // Asked about once.
    assert_eq!(
        hook(dir.path(), &["stop"], &stop(dir.path(), false)),
        (0, None)
    );
    // And never by a stop this hook already forced.
    say(dir.path(), "Always run cargo fmt before you commit.");
    assert_eq!(
        hook(dir.path(), &["stop"], &stop(dir.path(), true)),
        (0, None)
    );
}

#[test]
fn a_decision_is_offered_only_when_it_says_the_same_thing() {
    let dir = project("surface = \"on\"");
    let said = context(
        say(dir.path(), "Never use unwrap in library code.").1,
        "UserPromptSubmit",
    );
    assert!(!said.contains("design/exported-doc"), "{said}");
    if said.contains("Existing decisions") {
        assert!(said.contains("design/no-panic"), "{said}");
    }
}

#[test]
fn the_findings_a_session_introduces_and_resolves_become_signals() {
    let dir = text_project();
    pre(dir.path(), "small.txt");
    pre(dir.path(), "big.txt");
    fs::write(dir.path().join("small.txt"), BIG).unwrap();
    fs::write(dir.path().join("big.txt"), "a\n").unwrap();
    for file in ["small.txt", "big.txt"] {
        let payload = super::agent::edit(dir.path(), file);
        hook(dir.path(), &["post-tool-use"], &payload);
    }
    hook(dir.path(), &["stop"], &stop(dir.path(), false));

    let all = signals(dir.path());
    let rows = all["signals"].as_array().unwrap();
    let change = |what: &str| {
        rows.iter()
            .filter(|s| s["features"]["change"] == what)
            .collect::<Vec<_>>()
    };
    let introduced = change("introduced");
    let resolved = change("resolved");
    assert_eq!(
        introduced.len(),
        1,
        "once, however many hooks saw it: {all}"
    );
    assert_eq!(resolved.len(), 1, "{all}");
    for s in introduced.iter().chain(&resolved) {
        assert_eq!(s["class"], "finding");
        assert_eq!(s["by"]["type"], "SoftwareAgent");
    }
    assert_eq!(introduced[0]["rule"], "core/max-lines");
    assert!(
        introduced[0]["excerpt"]
            .as_str()
            .unwrap()
            .starts_with("introduced core/max-lines")
    );
    assert_eq!(all["counts"]["byClass"]["finding"], 2);
}

fn proposal(name: &str, requirement: &str, signal: Option<&str>) -> String {
    let derived = signal.map_or_else(String::new, |id| {
        format!("  provenance:\n    wasDerivedFrom: [\"signal:{id}\"]\n")
    });
    format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/{name}\nspec:\n  title: Library code does not unwrap\n  context: A panic in a library surprises its callers.\n  scope: {{ subject: symbol }}\n  requirement: {requirement}\n{derived}"
    )
}

fn create(dir: &Path, document: &str) -> Value {
    let out = lighthouse(dir)
        .args(["decision", "create", "-", "--format", "json"])
        .write_stdin(document.to_owned())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn a_created_decision_is_proposed_tied_to_its_signals_and_checked_for_duplicates() {
    let dir = project("");
    say(dir.path(), DIRECTIVE);
    let id = signals(dir.path())["signals"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let requirement = "Library code MUST NOT call unwrap and returns a Result instead.";

    let first = create(
        dir.path(),
        &proposal("no-unwrap", requirement, Some(&id[..8])),
    );
    assert_eq!(first["id"], "local/no-unwrap");
    assert_eq!(first["signals"], json!([id]));
    assert!(first.get("duplicates").is_none(), "{first}");
    let written =
        fs::read_to_string(dir.path().join(".lighthouse/decisions/no-unwrap.yaml")).unwrap();
    assert!(written.contains("status: proposed"), "{written}");

    let after = signals(dir.path());
    assert_eq!(after["signals"][0]["status"], "recorded");
    assert!(after["signals"][0]["decision"].as_str().is_some());
    assert_eq!(after["measures"]["capture"]["hits"], 1);

    let shown = lighthouse(dir.path())
        .args(["decision", "show", "local/no-unwrap", "--format", "json"])
        .output()
        .unwrap();
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["support"]["signals"], 1);
    assert_eq!(shown["support"]["byClass"]["directive"]["person"], 1);
    assert!(
        shown["markdown"]
            .as_str()
            .unwrap()
            .contains("support: 1 signal(s)")
    );

    // The same requirement under another name succeeds and names the first.
    let second = create(
        dir.path(),
        &proposal("no-unwrap-in-libs", requirement, None),
    );
    assert_eq!(second["duplicates"][0]["id"], "local/no-unwrap");
    assert!(
        second["note"]
            .as_str()
            .unwrap()
            .contains("possible duplicate")
    );
    let measures = signals(dir.path())["measures"].clone();
    assert_eq!(measures["duplicate"]["hits"], 1);
    assert_eq!(measures["duplicate"]["of"], 2);
    assert_eq!(measures["ratification"]["hits"], 0);

    // Something else is no duplicate.
    let other = create(
        dir.path(),
        &proposal(
            "kebab-flags",
            "Command line flags MUST be spelled in kebab-case.",
            None,
        ),
    );
    assert!(other.get("duplicates").is_none(), "{other}");
}

#[test]
fn a_signal_is_dismissed_with_a_reason_and_by_whom() {
    let dir = project("");
    say(dir.path(), DIRECTIVE);
    let id = signals(dir.path())["signals"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    lighthouse(dir.path())
        .args(["decision", "signal", "dismiss", &id[..7]])
        .assert()
        .code(2);
    lighthouse(dir.path())
        .args(["decision", "signal", "dismiss", "0000000", "--reason", "x"])
        .assert()
        .code(2);
    let done = lighthouse(dir.path())
        .args([
            "decision",
            "signal",
            "dismiss",
            &id[..7],
            "--reason",
            "a request, not a rule",
            "--attributed-to",
            "person:ana",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    let done: Value = serde_json::from_slice(&done.stdout).unwrap();
    assert_eq!(done["status"], "dismissed");
    assert_eq!(done["reason"], "a request, not a rule");
    assert_eq!(done["settledBy"], "person:ana");
    // A dismissed signal is not asked about at a stop.
    let dir_on = project("surface = \"on\"");
    say(dir_on.path(), DIRECTIVE);
    let id = signals(dir_on.path())["signals"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    lighthouse(dir_on.path())
        .args(["decision", "signal", "dismiss", &id, "--reason", "no"])
        .assert()
        .success();
    assert_eq!(
        hook(dir_on.path(), &["stop"], &stop(dir_on.path(), false)),
        (0, None)
    );
}

#[test]
fn init_installs_the_prompt_and_session_hooks() {
    let dir = text_project();
    lighthouse(dir.path())
        .args(["init", "--agent", "claude-code"])
        .assert()
        .success();
    let settings: Value = serde_json::from_str(
        &fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    for (event, command) in [
        (
            "UserPromptSubmit",
            "lighthouse hook claude-code user-prompt-submit",
        ),
        ("SessionStart", "lighthouse hook claude-code session-start"),
    ] {
        assert_eq!(settings["hooks"][event][0]["hooks"][0]["command"], command);
    }
}
