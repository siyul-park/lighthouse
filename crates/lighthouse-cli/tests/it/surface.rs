//! The surface through the binary: the help of every command is golden, the
//! output schemas are the ones `schema` prints, and each exit code has a
//! case.

use std::path::Path;

use serde_json::{Value, json};

use crate::{
    checks::{lighthouse, project, trust, write},
    command_sarif::wrapping,
    mcp::{Client, text_project},
};

/// What `lighthouse <args>` printed on stdout.
fn printed(dir: &Path, args: &[&str]) -> String {
    let out = lighthouse(dir)
        .env("NO_COLOR", "1")
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// The subcommands a help page lists, without `help`.
fn subcommands(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_owned)
        .collect()
}

/// Every command path of the tree, parents before children.
fn paths(dir: &Path, prefix: &[String], found: &mut Vec<Vec<String>>) {
    let mut args: Vec<&str> = prefix.iter().map(String::as_str).collect();
    args.push("--help");
    for name in subcommands(&printed(dir, &args)) {
        let mut path = prefix.to_vec();
        path.push(name);
        found.push(path.clone());
        paths(dir, &path, found);
    }
}

#[test]
fn the_help_of_every_command_is_golden() {
    let dir = tempfile::tempdir().unwrap();
    let mut found = vec![Vec::new()];
    paths(dir.path(), &[], &mut found);
    assert!(found.len() > 25, "{found:?}");
    for path in found {
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let name = format!("help-{}", path.join("-"));
        insta::assert_snapshot!(name, printed(dir.path(), &args));
    }
}

#[test]
fn the_output_schema_of_every_tool_is_what_schema_prints() {
    let dir = text_project();
    let mut client = Client::start(dir.path());
    let tools = client.request("tools/list", json!({}));
    for tool in tools["result"]["tools"].as_array().unwrap() {
        let kind = tool["outputSchema"]["title"].as_str().unwrap();
        let printed: Value = serde_json::from_str(&printed(dir.path(), &["schema", kind])).unwrap();
        assert_eq!(tool["outputSchema"], printed, "{kind}");
    }
}

#[test]
fn exit_0_is_a_clean_run() {
    let dir = text_project();
    write(dir.path(), "a.txt", "fine\n");
    lighthouse(dir.path()).arg("check").assert().code(0);
}

#[test]
fn exit_1_is_a_finding_over_the_threshold() {
    let dir = project(&crate::checks::decision(
        "  check:\n    type: cel\n    select: file\n    where: 'file.lines >= 1'\n    message: 'a file'\n",
    ));
    lighthouse(dir.path()).arg("check").assert().code(1);
}

#[test]
fn exit_1_is_also_a_stale_doc_and_a_failed_test() {
    let dir = tempfile::tempdir().unwrap();
    lighthouse(dir.path())
        .args(["docs", "check"])
        .assert()
        .code(1);
}

#[test]
fn exit_2_is_a_usage_or_runtime_error() {
    let dir = text_project();
    lighthouse(dir.path())
        .args(["check", "--format", "xml"])
        .assert()
        .code(2);
    lighthouse(dir.path())
        .args(["decision", "show", "nope/nope"])
        .assert()
        .code(2);
}

#[test]
fn exit_3_is_an_incomplete_analysis() {
    let dir = project(&wrapping(""));
    write(dir.path(), "tools/lint.sh", "true\nexit 1\n");
    trust(dir.path());
    lighthouse(dir.path()).arg("check").assert().code(3);
}
