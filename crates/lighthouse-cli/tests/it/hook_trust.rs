//! The hooks of a project the user has not trusted: the session is told once,
//! without a block, and hears nothing more of it.

use std::fs;

use serde_json::json;
use tempfile::TempDir;

use crate::agent::{BIG, edit, hook, pre_payload, stop};

/// A project that turns the sandbox off, which needs trust to be checked.
fn untrusted() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project(
            "[sandbox]\nenabled = false\n[rules]\n\"core/max-lines\" = { level = \"error\", options = { max = 3 } }\n",
        ),
    )
    .unwrap();
    fs::write(dir.path().join("big.txt"), BIG).unwrap();
    dir
}

#[test]
fn an_untrusted_project_is_told_once_per_session_without_a_block() {
    let dir = untrusted();

    let (code, told) = hook(dir.path(), &["post-tool-use"], &edit(dir.path(), "big.txt"));
    let told = told.expect("the first hook says the project is not checked");
    assert_eq!(code, 0);
    assert!(told.get("decision").is_none(), "{told}");
    let text = told["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        text.contains("not checking") && text.contains("lighthouse trust"),
        "{text}"
    );

    let again = hook(dir.path(), &["post-tool-use"], &edit(dir.path(), "big.txt"));
    assert_eq!(again, (0, None));
    assert_eq!(
        hook(dir.path(), &["stop"], &stop(dir.path(), false)),
        (0, None)
    );
    assert_eq!(
        hook(
            dir.path(),
            &["pre-tool-use"],
            &pre_payload(dir.path(), "big.txt")
        ),
        (0, None)
    );

    let mut other = edit(dir.path(), "big.txt");
    other["session_id"] = json!("s2");
    let (code, told) = hook(dir.path(), &["post-tool-use"], &other);
    assert_eq!(code, 0);
    assert!(told.is_some_and(|t| t.get("decision").is_none()));
}

#[test]
fn stop_never_blocks_an_untrusted_project() {
    let dir = untrusted();

    let (code, told) = hook(dir.path(), &["stop"], &stop(dir.path(), false));

    assert_eq!(code, 0);
    let told = told.expect("the first hook says the project is not checked");
    assert!(told.get("decision").is_none(), "{told}");
    assert_eq!(
        hook(dir.path(), &["stop"], &stop(dir.path(), false)),
        (0, None)
    );
}

#[test]
fn the_prompt_and_session_start_hooks_work_without_trust_and_keep_the_notice() {
    let dir = untrusted();
    let prompt = json!({
        "session_id": "s1",
        "cwd": dir.path().to_str().unwrap(),
        "hook_event_name": "UserPromptSubmit",
        "prompt": "use a smaller file",
    });
    let start = json!({
        "session_id": "s1",
        "cwd": dir.path().to_str().unwrap(),
        "hook_event_name": "SessionStart",
        "source": "startup",
    });

    assert_eq!(hook(dir.path(), &["user-prompt-submit"], &prompt).0, 0);
    assert_eq!(hook(dir.path(), &["session-start"], &start).0, 0);
    let (_, told) = hook(dir.path(), &["post-tool-use"], &edit(dir.path(), "big.txt"));
    assert!(told.is_some());
}
