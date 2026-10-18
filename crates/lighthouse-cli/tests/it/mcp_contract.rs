//! The tool contract of the MCP server: the tool list is golden, and what a
//! tool returns is its `outputSchema`'s kind, equal to the CLI's JSON.

use jsonschema::Validator;
use serde_json::{Value, json};

use lighthouse_store::Records;

use crate::{
    attach::{DECISION, MODEL, mislabelled},
    mcp::{Client, rust_project, text_project},
    models::project,
    models_plugin::{declared, label_table},
};

/// The `tools/list` result.
fn listed(client: &mut Client) -> Vec<Value> {
    let reply = client.request("tools/list", json!({}));
    reply["result"]["tools"].as_array().unwrap().clone()
}

/// The raw `tools/call` result of `name`.
fn called(client: &mut Client, name: &str, arguments: Value) -> Value {
    let reply = client.request(
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    );
    reply["result"].clone()
}

/// Checks `result` against the `outputSchema` of the tool `name`: the
/// structured content validates, and the text block is its serialization.
fn conforms(tools: &[Value], name: &str, result: &Value) {
    assert_ne!(result["isError"], json!(true), "{name}: {result}");
    let tool = tools.iter().find(|t| t["name"] == name).unwrap();
    let validator: Validator =
        jsonschema::validator_for(&tool["outputSchema"]).unwrap_or_else(|e| panic!("{name}: {e}"));
    let structured = &result["structuredContent"];
    let errors: Vec<String> = validator
        .iter_errors(structured)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{name}: {errors:?}\n{structured}");
    assert_eq!(
        result["content"][0]["text"].as_str().unwrap(),
        structured.to_string(),
        "{name}: the text block is the serialized structured content"
    );
}

#[test]
fn the_tool_list_is_golden() {
    let dir = text_project();
    let mut client = Client::start(dir.path());
    let mut tools = listed(&mut client);
    tools.sort_by_key(|t| t["name"].as_str().unwrap().to_owned());
    assert_eq!(tools.len(), 17);
    for tool in tools {
        let name = format!("tool-{}", tool["name"].as_str().unwrap());
        insta::assert_snapshot!(name, serde_json::to_string_pretty(&tool).unwrap());
    }
}

#[test]
fn every_tool_has_title_schemas_and_annotations() {
    let dir = text_project();
    let mut client = Client::start(dir.path());
    for tool in listed(&mut client) {
        let name = tool["name"].as_str().unwrap();
        assert!(
            tool["title"].as_str().is_some_and(|t| !t.is_empty()),
            "{name}"
        );
        assert!(
            tool["description"].as_str().unwrap().len() >= tool["title"].as_str().unwrap().len()
        );
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        assert_eq!(tool["outputSchema"]["type"], "object", "{name}");
        let hints = &tool["annotations"];
        for hint in [
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ] {
            assert!(hints[hint].is_boolean(), "{name} {hint}");
        }
    }
}

#[test]
fn structured_content_conforms_to_the_output_schema_of_every_reading_tool() {
    let dir = rust_project();
    let mut client = Client::start(dir.path());
    let tools = listed(&mut client);

    let check = called(&mut client, "check", json!({}));
    conforms(&tools, "check", &check);
    let full = called(&mut client, "check", json!({ "detail": "full" }));
    conforms(&tools, "check", &full);
    let fingerprint = full["structuredContent"]["findings"][0]["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let prefix = &fingerprint[..7];

    for (name, arguments) in [
        ("decision_list", json!({})),
        ("decision_list", json!({ "all": true })),
        ("decision_show", json!({ "id": "design/exported-doc" })),
        ("decision_test", json!({ "decision": ["design/comments"] })),
        ("review_list", json!({})),
        ("review_list", json!({ "detail": "full" })),
        ("review_show", json!({ "fingerprint": prefix })),
        ("review_history", json!({ "fingerprint": prefix })),
        ("model_list", json!({})),
        (
            "fix",
            json!({ "decision": ["design/comments"], "dryRun": true }),
        ),
    ] {
        let result = called(&mut client, name, arguments);
        conforms(&tools, name, &result);
    }

    let resolved = called(
        &mut client,
        "review_resolve",
        json!({ "fingerprint": prefix, "judgment": "pass" }),
    );
    conforms(&tools, "review_resolve", &resolved);
}

#[test]
fn authoring_tools_return_their_report_kind() {
    let dir = text_project();
    let mut client = Client::start(dir.path());
    let tools = listed(&mut client);
    let document = json!({
        "kind": "Decision",
        "metadata": { "name": "local/short-notes" },
        "spec": {
            "title": "Notes stay short",
            "context": "Notes are read in one glance.",
            "scope": { "subject": "file" },
            "requirement": "A note file MUST NOT exceed three lines.",
            "severity": "error",
            "check": { "type": "cel", "select": "file", "where": "file.lines > 3",
                       "message": "{{ file.path }} is too long", "evidence": { "path": "file.path" } },
            "examples": [
                { "name": "long", "language": "text", "kind": "invalid",
                  "files": [{ "path": "a.txt", "body": "a\nb\nc\nd\n" }], "expect": [{ "line": 1 }] },
                { "name": "short", "language": "text", "kind": "valid",
                  "files": [{ "path": "a.txt", "body": "a\n" }] }
            ]
        }
    });
    let created = called(
        &mut client,
        "decision_create",
        json!({ "document": document }),
    );
    conforms(&tools, "decision_create", &created);
    let updated = called(
        &mut client,
        "decision_update",
        json!({ "document": document }),
    );
    conforms(&tools, "decision_update", &updated);
}

#[test]
fn an_unknown_tool_is_a_protocol_error_and_a_failed_one_an_error_result() {
    let dir = text_project();
    let mut client = Client::start(dir.path());
    let unknown = client.request("tools/call", json!({ "name": "nope", "arguments": {} }));
    assert_eq!(unknown["error"]["code"], -32602, "{unknown}");
    assert!(unknown.get("result").is_none());

    let failed = called(&mut client, "decision_show", json!({ "id": "nope/nope" }));
    assert_eq!(failed["isError"], true, "{failed}");
    assert!(
        failed["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("nope/nope")
    );
}

#[test]
fn a_refused_attach_is_a_result_and_a_detach_is_attributed_to_the_agent() {
    let dir = project(&declared());
    mislabelled(dir.path());
    let mut client = Client::trusting(dir.path());
    let tools = listed(&mut client);
    let arguments = json!({ "id": DECISION, "model": MODEL, "labels": "labels.tsv" });

    let refused = called(&mut client, "model_attach", arguments.clone());
    conforms(&tools, "model_attach", &refused);
    assert_eq!(refused["isError"], json!(false), "{refused}");
    let reasons = refused["structuredContent"]["refused"].to_string();
    assert!(reasons.contains("misses the policy"), "{reasons}");
    assert!(Records::read(dir.path()).unwrap().revisions.is_empty());

    label_table(dir.path());
    let attached = called(&mut client, "model_attach", arguments);
    conforms(&tools, "model_attach", &attached);
    assert!(attached["structuredContent"]["revision"].is_string());

    let detached = called(&mut client, "model_detach", json!({ "id": DECISION }));
    conforms(&tools, "model_detach", &detached);
    let records = Records::read(dir.path()).unwrap();
    assert_eq!(records.revisions.len(), 2);
    for revision in &records.revisions {
        assert_eq!(revision.spec.approval.id.as_deref(), Some("test-agent"));
    }
    assert!(records.attached().is_empty());

    let nothing = called(&mut client, "model_detach", json!({ "id": DECISION }));
    assert_eq!(nothing["isError"], json!(true), "{nothing}");
}
