//! What an agent did, in the fix report: the events of a `command` agent's
//! stdout mapped by the provider's own `events`, and the events an `rpc`
//! agent plugin returns with its edit.

use serde_json::{Value, json};

use crate::{
    fix_edit::{ALLOWED, edited, lib},
    mcp::Client,
    models::project,
    models_plugin::{lighthouse, with_plugin},
};

/// An agent that renames the banned function, and prints what it did as JSON
/// lines of a shape of its own: it opens the file of the finding (which the
/// brief includes) and another one (which it does not), looks for a pattern,
/// lists a directory, edits, and runs a command.
const AGENT: &str =
    "sed 's/bad_one/fixed_one/' src/lib.rs > src/lib.rs.new && mv src/lib.rs.new src/lib.rs
echo \"{\\\"did\\\":\\\"open\\\",\\\"file\\\":\\\"$PWD/src/lib.rs\\\"}\"
echo \"{\\\"did\\\":\\\"open\\\",\\\"file\\\":\\\"$PWD/src/other.rs\\\"}\"
echo \"{\\\"did\\\":\\\"open\\\",\\\"file\\\":\\\"$PWD/src/other.rs\\\"}\"
echo '{\"did\":\"find\",\"what\":\"bad_one\"}'
echo \"{\\\"did\\\":\\\"ls\\\",\\\"file\\\":\\\"$PWD/src\\\"}\"
echo \"{\\\"did\\\":\\\"write\\\",\\\"file\\\":\\\"$PWD/src/lib.rs\\\"}\"
echo '{\"did\":\"sh\",\"cmd\":\"cargo check\"}'
echo '{\"did\":\"think\"}'
echo 'not json at all'
";

const EVENTS: &str = "[models.providers.agent.events]
select = 'line.?did.orValue(\"\") != \"\"'
tool = 'tools[line.did]'
path = 'line.?file.orValue(\"\")'
pattern = 'line.?what.orValue(\"\")'
command = 'line.?cmd.orValue(\"\")'
tools = { open = \"read\", find = \"search\", ls = \"list\", write = \"edit\", sh = \"run\" }
";

fn rpc_agent() -> tempfile::TempDir {
    let dir = project(&format!(
        "[models]\nagent = \"fake/m\"\n[models.providers.fake]\nplugin = \"fake-model\"\n{ALLOWED}"
    ));
    with_plugin(dir.path());
    crate::cache_fixtures::write(dir.path(), "src/lib.rs", "pub fn bad_one() {}\n");
    // The file the fake agent says it read beyond the brief: a path that
    // names nothing in the directory is not reported.
    crate::cache_fixtures::write(dir.path(), "other.go", "package other\n");
    dir
}

fn batch_of(fixed: &Value) -> &Value {
    &fixed["usage"]["agent"]["batches"][0]
}

#[test]
fn what_a_command_agent_did_is_in_the_report_from_the_mapping_its_provider_declares() {
    let (dir, _agent) = edited(AGENT, "", &format!("{EVENTS}{ALLOWED}"));

    let fixed = Client::trusting(dir.path())
        .tool("fix", json!({"decision": ["local/naming-ban"]}))
        .unwrap();

    assert_eq!(fixed["applied"].as_array().unwrap().len(), 1, "{fixed}");
    let batch = batch_of(&fixed);
    assert_eq!(batch["file"], "src/lib.rs", "{fixed}");
    assert_eq!(batch["reads"], json!(["src/other.rs"]), "{fixed}");
    assert_eq!(
        batch["searches"],
        json!([
            {"tool": "search", "pattern": "bad_one"},
            {"tool": "list", "path": "src"},
        ])
    );
    assert_eq!(
        batch["calls"],
        json!({"read": 3, "search": 1, "list": 1, "edit": 1, "run": 1})
    );
}

#[test]
fn the_events_are_a_property_of_the_sarif_run_and_never_a_reason_to_decline() {
    let (dir, _agent) = edited(AGENT, "", &format!("{EVENTS}{ALLOWED}"));

    let out = lighthouse(
        dir.path(),
        &["check", "--fix", "--format", "sarif", "--no-store"],
    );

    let log: Value = serde_json::from_slice(&out.stdout).unwrap();
    let agent = &log["runs"][0]["properties"]["agent"];
    assert_eq!(agent["runs"], 1, "{log}");
    assert_eq!(agent["batches"][0]["reads"], json!(["src/other.rs"]));
    assert!(lib(&dir).contains("fixed_one"), "the fix is still applied");
}

#[test]
fn a_provider_without_a_mapping_reports_no_events() {
    let (dir, _agent) = edited(AGENT, "", ALLOWED);

    let fixed = Client::trusting(dir.path())
        .tool("fix", json!({"decision": ["local/naming-ban"]}))
        .unwrap();

    assert_eq!(fixed["usage"]["agent"]["runs"], 1, "{fixed}");
    assert!(fixed["usage"]["agent"].get("batches").is_none(), "{fixed}");
}

#[test]
fn what_an_rpc_agent_plugin_returns_is_in_the_report() {
    let dir = rpc_agent();

    let fixed = Client::trusting(dir.path())
        .tool("fix", json!({"decision": ["local/naming-ban"]}))
        .unwrap();

    let batch = batch_of(&fixed);
    assert_eq!(batch["reads"], json!(["other.go"]), "{fixed}");
    assert_eq!(
        batch["calls"],
        json!({"read": 2, "search": 1, "list": 1, "edit": 1, "run": 1}),
        "{fixed}"
    );
    assert_eq!(batch["searches"][0]["pattern"], "func Parse", "{fixed}");
}
