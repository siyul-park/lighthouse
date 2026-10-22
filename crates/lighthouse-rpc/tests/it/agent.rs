//! The agent of a plugin: it runs in a process of its own, sandboxed like a
//! command agent, and what it reports is read as data.

use std::{fs, path::Path, time::Duration};

use lighthouse_plugin::{EditRequest, Edited, ModelClient, Plugin, Tool, ToolEvent};
use lighthouse_process::Policy;
use lighthouse_rpc::RpcPlugin;

const T: Duration = Duration::from_secs(60);

/// The agent of the `fake-model` plugin, confined to `root`'s sandbox.
fn agent(root: &Path) -> Box<dyn ModelClient> {
    let dir = lighthouse_test_support::fake_model();
    let found = lighthouse_rpc::load(&dir).unwrap();
    let plugin = RpcPlugin::connect(&found, &dir, T)
        .unwrap()
        .confined(&Policy::default(), root);
    plugin.models().remove(0)
}

fn ask(prompt: &str, scratch: &Path) -> Edited {
    agent(scratch)
        .edit(&EditRequest {
            prompt,
            dir: scratch,
            model: "m",
        })
        .unwrap()
}

#[test]
fn a_plugin_agent_that_writes_outside_its_directory_is_denied() {
    let scratch = tempfile::tempdir().unwrap();
    let real = tempfile::tempdir().unwrap();
    let target = real.path().join("escaped.txt");

    let edited = ask(&format!("outside:{}", target.display()), scratch.path());

    assert!(edited.said.contains("denied"), "{}", edited.said);
    assert!(!target.exists(), "the real tree is unchanged");
}

#[test]
fn a_plugin_agent_writes_the_directory_it_is_given() {
    let scratch = tempfile::tempdir().unwrap();

    ask("fix it", scratch.path());

    assert_eq!(
        fs::read_to_string(scratch.path().join("edited.txt")).unwrap(),
        "fix it"
    );
}

#[test]
fn a_plugin_agent_reports_what_it_did_relative_to_its_directory() {
    let scratch = tempfile::tempdir().unwrap();

    let edited = ask("fix it", scratch.path());

    let tools: Vec<Tool> = edited.events.iter().map(|e| e.tool).collect();
    assert_eq!(
        tools,
        [
            Tool::Read,
            Tool::Read,
            Tool::Search,
            Tool::List,
            Tool::Edit,
            Tool::Run
        ]
    );
    assert_eq!(edited.said, "m edited");
    assert_eq!(edited.incomplete, None);
}

#[test]
fn a_plugin_agent_with_absolute_paths_is_made_relative_and_what_is_outside_is_marked() {
    let scratch = tempfile::tempdir().unwrap();
    fs::create_dir(scratch.path().join("src")).unwrap();
    fs::write(scratch.path().join("src/lib.rs"), "pub fn x() {}\n").unwrap();

    let edited = ask("absolute", scratch.path());

    let seen: Vec<(Option<&str>, bool)> = edited
        .events
        .iter()
        .map(|e: &ToolEvent| (e.path.as_deref(), e.outside))
        .collect();
    assert_eq!(
        seen,
        [
            (Some("src/lib.rs"), false),
            (Some("src/lib.rs"), false),
            (None, true),
            (None, true),
        ]
    );
}
