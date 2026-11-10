//! The MCP server keeps the project loaded between calls: a call answers as a
//! command line run would, whatever changed between the calls.

use std::{fs, path::Path, process::Command};

use serde_json::{Value, json};

use crate::mcp::{Client, notes, rust_project};

/// What `lighthouse check --format json` prints in `dir`.
fn cli(dir: &Path) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_lighthouse"))
        .args(["check", "--format", "json"])
        .current_dir(dir)
        .env("XDG_CONFIG_HOME", dir.join(".home"))
        .env("XDG_DATA_HOME", dir.join(".home"))
        .env_remove("LIGHTHOUSE_TRUST")
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Whether the command line run in `dir` loads the project.
fn cli_ok(dir: &Path) -> bool {
    Command::new(env!("CARGO_BIN_EXE_lighthouse"))
        .args(["check", "--format", "json"])
        .current_dir(dir)
        .env("XDG_CONFIG_HOME", dir.join(".home"))
        .env("XDG_DATA_HOME", dir.join(".home"))
        .output()
        .unwrap()
        .status
        .code()
        .is_some_and(|code| code < 2)
}

#[test]
fn every_check_of_one_server_is_a_command_line_run() {
    let dir = rust_project();
    let mut client = Client::start(dir.path());
    for round in 0..3 {
        if round == 2 {
            fs::write(dir.path().join("src/lib.rs"), "pub fn run() {}\n").unwrap();
        }
        let served = client.tool("check", json!({})).unwrap();
        assert_eq!(served, cli(dir.path()), "round {round}");
    }
}

/// The configuration with the rule `core/max-lines` at `max`.
fn lines_config(max: usize) -> String {
    lighthouse_test_support::project(&format!(
        "plugins = []\n[rules]\n\"core/max-lines\" = {{ level = \"warn\", options = {{ max = {max} }} }}\n\"local/short-notes\" = \"error\"\n"
    ))
}

/// A project with the rule `core/max-lines` at `max`, over one file of eight lines.
fn lines_project(max: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("lighthouse.toml"), lines_config(max)).unwrap();
    fs::write(dir.path().join("big.txt"), "a\nb\nc\nd\ne\nf\ng\nh\n").unwrap();
    dir
}

#[test]
fn an_edit_to_the_config_or_the_decisions_between_calls_is_seen() {
    let dir = lines_project(6);
    let config = dir.path().join("lighthouse.toml");
    let mut client = Client::start(dir.path());
    // `local/short-notes` is named before it exists.
    let before = client.tool("check", json!({}));
    assert_eq!(before.is_ok(), cli_ok(dir.path()));

    let decisions = dir.path().join(".lighthouse/decisions");
    fs::create_dir_all(&decisions).unwrap();
    let mut document = notes("local/short-notes", "a\nb\nc\nd\n");
    document["apiVersion"] = json!("lighthouse/v1alpha1");
    fs::write(decisions.join("short-notes.json"), document.to_string()).unwrap();
    let created = client.tool("check", json!({})).unwrap();
    assert_eq!(created, cli(dir.path()));
    assert_eq!(created["counts"]["warn"], 1, "{created}");

    fs::write(&config, lines_config(9)).unwrap();
    let raised = client.tool("check", json!({})).unwrap();
    assert_eq!(raised, cli(dir.path()));
    assert_ne!(raised, created);

    fs::write(&config, "[spec\n").unwrap();
    assert!(client.tool("check", json!({})).is_err());

    fs::write(&config, lines_config(6)).unwrap();
    assert_eq!(client.tool("check", json!({})).unwrap(), created);
}

#[test]
fn a_provider_that_was_killed_starts_again() {
    let dir = rust_project();
    let mut client = Client::start(dir.path());
    let first = client.tool("check", json!({})).unwrap();
    assert_eq!(first["status"], "findings");

    let children = Command::new("pgrep")
        .args(["-P", &client.pid().to_string()])
        .output()
        .unwrap();
    let pids = String::from_utf8(children.stdout).unwrap();
    assert!(!pids.trim().is_empty(), "the provider is a child process");
    for pid in pids.split_whitespace() {
        Command::new("kill").args(["-9", pid]).status().unwrap();
    }

    let source = dir.path().join("src/lib.rs");
    fs::write(
        &source,
        "pub fn run(x: Option<u8>) -> u8 {\n    x.unwrap() + 1\n}\n",
    )
    .unwrap();
    let gone = client.tool("check", json!({})).unwrap();
    assert_eq!(gone["status"], "incomplete", "{gone}");

    fs::write(
        &source,
        "pub fn run(x: Option<u8>) -> u8 {\n    x.unwrap() + 2\n}\n",
    )
    .unwrap();
    let again = client.tool("check", json!({})).unwrap();
    assert_eq!(again, cli(dir.path()));
    assert_eq!(again["status"], "findings");
}
