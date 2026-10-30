//! How a decision's fix is described: the same fields for every provider.

use serde_json::{Value, json};

use crate::{
    cache_fixtures::write,
    fix::{lighthouse, read},
    fx::project,
    mcp::Client,
};

fn listed(client: &mut Client) -> Value {
    let list = client.tool("decision_list", json!({})).unwrap();
    list["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["id"].as_str().unwrap().to_owned(), d.clone()))
        .collect::<serde_json::Map<String, Value>>()
        .into()
}

/// How a decision is fixed: the step that writes a safe fix and previews an
/// unsafe one, never one that writes it unseen.
fn call(id: &str) -> Value {
    json!({
        "apply": format!("lighthouse check --fix --decision {id}"),
        "tool": {"name": "fix", "arguments": {"decision": [id]}}
    })
}

/// The `fix` of a descriptor: `by`, `safety` and `written` as given, and the
/// call that applies it.
fn fix_json(by: &str, safety: &str, runs: bool, why: Option<&str>, id: &str) -> Value {
    let mut fix = json!({"by": by, "safety": safety, "runs": runs});
    if let Some(why) = why {
        fix["why"] = json!(why);
    }
    if safety == "unsafe" {
        fix["written"] = json!(false);
    }
    for (key, value) in call(id).as_object().unwrap() {
        fix[key] = value.clone();
    }
    fix
}

#[test]
fn the_decision_list_describes_every_provider_the_same_way() {
    let (dir, _agent) = project("");

    let decisions = listed(&mut Client::trusting(dir.path()));

    let cel = json!({"by": "cel", "runs": true});
    for (id, by, safety) in [
        ("local/ops-safe", "cel", "safe"),
        ("local/ops-unsafe", "cel", "unsafe"),
        ("local/command-safe", "command", "safe"),
        ("local/command-unsafe", "command", "unsafe"),
        ("local/model", "model", "unsafe"),
    ] {
        assert_eq!(decisions[id]["check"], cel, "{id}");
        assert_eq!(
            decisions[id]["fix"],
            fix_json(by, safety, true, None, id),
            "{id}"
        );
    }
}

#[test]
fn an_untrusted_project_says_why_its_programs_do_not_run() {
    let (dir, _agent) = project("");

    let decisions = listed(&mut Client::start(dir.path()));

    assert_eq!(decisions["local/ops-safe"]["fix"]["runs"], true);
    for (id, by, safety) in [
        ("local/command-safe", "command", "safe"),
        ("local/command-unsafe", "command", "unsafe"),
        ("local/model", "model", "unsafe"),
    ] {
        assert_eq!(
            decisions[id]["fix"],
            fix_json(by, safety, false, Some("untrusted project"), id),
            "{id}"
        );
    }
}

#[test]
fn a_decision_a_model_may_fix_without_an_agent_bound_says_no_edit_binding() {
    let (dir, _agent) = project("");
    let toml = read(&dir, "lighthouse.toml");
    let bare = toml.replace("\"agent\" = \"agent/m\", ", "");
    assert_ne!(bare, toml);
    write(dir.path(), "lighthouse.toml", &bare);

    let decisions = listed(&mut Client::trusting(dir.path()));

    assert_eq!(
        decisions["local/model"]["fix"],
        fix_json(
            "model",
            "unsafe",
            false,
            Some("no agent binding"),
            "local/model"
        )
    );
}

#[test]
fn a_decision_listed_for_a_model_without_a_fix_of_its_own_gets_one_only_when_listed() {
    let (dir, _agent) = project("");
    let toml = read(&dir, "lighthouse.toml");
    write(
        dir.path(),
        "lighthouse.toml",
        &toml.replace("\"decisions\" = [\"local/model\"]", "\"decisions\" = []"),
    );

    let decisions = listed(&mut Client::trusting(dir.path()));

    assert!(decisions["local/model"].get("fix").is_none());
    assert_eq!(decisions["local/ops-safe"]["fix"]["by"], "cel");
}

#[test]
fn decision_show_gives_the_whole_descriptor() {
    let (dir, _agent) = project("");
    let mut client = Client::trusting(dir.path());

    let safe = client
        .tool("decision_show", json!({"id": "local/ops-safe"}))
        .unwrap();
    let unsafe_fix = client
        .tool("decision_show", json!({"id": "local/ops-unsafe"}))
        .unwrap();

    assert_eq!(safe["check"], json!({"by": "cel", "runs": true}));
    assert_eq!(
        safe["fix"],
        fix_json("cel", "safe", true, None, "local/ops-safe")
    );
    assert!(
        safe.get("brief").is_none(),
        "a safe fix that runs needs no brief"
    );
    assert_eq!(unsafe_fix["brief"], true, "an unsafe fix has a brief");
}

#[test]
fn a_finding_of_a_decision_without_an_edit_agent_has_a_brief_and_no_running_fix() {
    let (dir, _agent) = project("");
    let toml = read(&dir, "lighthouse.toml");
    write(
        dir.path(),
        "lighthouse.toml",
        &toml.replace("\"agent\" = \"agent/m\", ", ""),
    );
    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(["check", "--format", "json", "--decision", "local/model"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let group = &report["groups"][0];
    let fingerprint = group["files"]["model.txt"][0][2]
        .as_str()
        .unwrap()
        .to_owned();

    let shown = Client::trusting(dir.path())
        .tool("review_show", json!({"fingerprint": fingerprint}))
        .unwrap();

    assert_eq!(group["fix"]["runs"], false, "{group}");
    assert_eq!(group["fix"]["why"], "no agent binding");
    assert_eq!(group["brief"], true);
    assert_eq!(shown["fix"]["why"], "no agent binding");
    assert!(
        shown["brief"]
            .as_str()
            .unwrap()
            .contains("Decision: local/model")
    );
}

#[test]
fn a_model_without_budget_says_so_in_the_descriptor() {
    let (dir, _agent) = project("");
    let toml = read(&dir, "lighthouse.toml");
    write(
        dir.path(),
        "lighthouse.toml",
        &toml.replace("\"models\" = { ", "\"models\" = { \"budget\" = 0, "),
    );

    let decisions = listed(&mut Client::trusting(dir.path()));

    assert_eq!(
        decisions["local/model"]["fix"],
        fix_json("model", "unsafe", false, Some("no budget"), "local/model")
    );
    assert_eq!(decisions["local/ops-safe"]["fix"]["runs"], true);
}
