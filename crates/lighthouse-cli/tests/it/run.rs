//! Running a command and a tool in process, against a project file the
//! caller names. The project root is the current directory, which holds no
//! decisions of its own.

use std::path::PathBuf;

use clap::FromArgMatches;
use lighthouse_cli::surface::{Action, Context, args::CheckArgs, cli::command, tools::tools};
use lighthouse_session::Session;
use serde_json::{Value, json};

/// A context over a project that enables the `core` pack, and its directory.
fn core_project() -> (tempfile::TempDir, Context) {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("lighthouse.toml");
    std::fs::write(
        &config,
        lighthouse_test_support::project("plugins = [\"core\"]\n"),
    )
    .unwrap();
    (dir, Context::person(Some(config)))
}

#[test]
fn run() {
    let (_dir, ctx) = core_project();
    let matches = command()
        .try_get_matches_from(["decision", "list", "--format", "json"])
        .unwrap();
    let action = Action::from_arg_matches(&matches).unwrap();
    let reply = lighthouse_cli::surface::run(&ctx, &action).unwrap();
    assert_eq!(reply.code, 0);
    assert!(reply.stdout.starts_with('{') && reply.stdout.ends_with("}\n"));
    assert_eq!(
        serde_json::from_str::<Value>(&reply.stdout).unwrap(),
        reply.value
    );
}

#[test]
fn call() {
    let (_dir, ctx) = core_project();
    assert!(
        lighthouse_cli::surface::tools::call(&ctx, "nope", json!({})).is_none(),
        "no such tool"
    );
    let reply = lighthouse_cli::surface::tools::call(&ctx, "decision_list", Value::Null)
        .unwrap()
        .unwrap();
    assert!(reply.value.is_object(), "{reply:?}");
    let error = lighthouse_cli::surface::tools::call(&ctx, "decision_list", json!({"nope": 1}))
        .unwrap()
        .unwrap_err();
    assert!(
        error.to_string().contains("unknown field `nope`"),
        "{error}"
    );
}

#[test]
fn schema() {
    let schema = lighthouse_cli::surface::tools::schema::<CheckArgs>();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema.get("$schema").is_none() && schema.get("title").is_none());
    assert!(schema["properties"]["paths"].is_object());
}

#[test]
fn tool_open_world() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::bare(PathBuf::from(dir.path())).unwrap();
    for tool in tools() {
        assert!(
            !tool.open_world(&session),
            "{}: no remote model is bound in a bare project",
            tool.name
        );
    }
}
