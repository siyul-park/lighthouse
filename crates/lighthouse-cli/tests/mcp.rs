//! The MCP server through the binary: the handshake, the tool list, a check of
//! a Rust project, the review round trip and rule authoring with its gate.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use serde_json::{Value, json};
use tempfile::TempDir;

const HELPER: &str = "pub fn run(x: u8) -> u8 {\n    clamp(x) + 1\n}\n\nfn clamp(x: u8) -> u8 {\n    if x > 10 { 10 } else { x }\n}\n";

/// A `lighthouse mcp` process spoken to in newline-delimited JSON-RPC.
struct Client {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Client {
    fn start(dir: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lighthouse"))
            .arg("mcp")
            .current_dir(dir)
            .env_remove("LIGHTHOUSE_REVIEWER")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut client = Self {
            child,
            input,
            output,
            next: 0,
        };
        let init = client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test-agent", "version": "1" }
            }),
        );
        assert!(
            init["result"]["capabilities"]["tools"].is_object(),
            "{init}"
        );
        client.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        client
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.input, "{message}").unwrap();
        self.input.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            assert!(
                self.output.read_line(&mut line).unwrap() > 0,
                "server closed"
            );
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    /// The JSON a tool returned, or its error text.
    fn tool(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        let reply = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        if result["isError"] == json!(true) {
            return Err(text.to_owned());
        }
        Ok(serde_json::from_str(text).unwrap_or_else(|_| panic!("{reply}")))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn rust_project() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let plugin = lighthouse_testkit::lang_rust();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_testkit::project(&format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"design\"]\nextends = [\"design/recommended\", \"design/strict\"]\n",
            plugin.to_str().unwrap()
        )),
    )
    .unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), HELPER).unwrap();
    dir
}

fn text_project() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_testkit::project("plugins = [\"core\"]\nextends = [\"core/recommended\"]\n"),
    )
    .unwrap();
    dir
}

const LONG_FILE_SPEC: &str = r#"
title: Notes stay short
intent: Notes are read in one glance.
scope: { subject: file }
requirement: A note file MUST NOT exceed three lines.
severity: error
evidence: [path]
check:
  type: cel
  select: file
  where: 'file.lines > 3'
  message: '{{ file.path }} is too long'
  evidence:
    path: file.path
"#;

fn examples(invalid_body: &str) -> Value {
    json!([
        { "name": "long", "language": "text", "kind": "invalid",
          "files": [{ "path": "a.txt", "body": invalid_body }], "expect": [{ "line": 1 }] },
        { "name": "short", "language": "text", "kind": "valid",
          "files": [{ "path": "a.txt", "body": "a\n" }] }
    ])
}

#[test]
fn the_server_lists_its_tools_and_resources() {
    let dir = text_project();
    let mut client = Client::start(dir.path());
    let tools = client.request("tools/list", json!({}));
    let mut names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            assert_eq!(t["inputSchema"]["type"], "object");
            t["name"].as_str().unwrap()
        })
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "check",
            "decision_create",
            "decision_list",
            "decision_test",
            "decision_update",
            "explain",
            "fix",
            "review_history",
            "review_resolve",
            "review_tasks"
        ]
    );
    let reserved = client.tool("decision_similar", json!({}));
    assert!(reserved.unwrap_err().contains("reserved"));
    let renamed = client.tool("rule_list", json!({}));
    assert!(renamed.unwrap_err().contains("decision_list"));

    let resources = client.request("resources/list", json!({}));
    let uris: Vec<&str> = resources["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert_eq!(uris, ["lighthouse://catalog", "lighthouse://config"]);
    let pattern = client.request(
        "resources/read",
        json!({ "uri": "lighthouse://decisions/core/max-file-lines" }),
    );
    let text = pattern["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(text.contains("max-file-lines"), "{text}");
    let config = client.request("resources/read", json!({ "uri": "lighthouse://config" }));
    assert!(
        config["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("core/max-file-lines")
    );
    let missing = client.request(
        "resources/read",
        json!({ "uri": "lighthouse://decisions/nope/nope" }),
    );
    assert!(missing["error"].is_object(), "{missing}");
}

#[test]
fn check_reports_findings_and_never_calls_an_incomplete_run_clean() {
    let dir = rust_project();
    let mut client = Client::start(dir.path());
    let found = client.tool("check", json!({})).unwrap();
    assert_eq!(found["status"], "findings");
    let rules: Vec<&str> = found["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["rule"].as_str().unwrap())
        .collect();
    assert!(rules.contains(&"design/exported-doc"), "{rules:?}");
    assert!(found["summary"]["warnings"].as_u64().unwrap() >= 1);

    let limited = client.tool("check", json!({ "limit": 1 })).unwrap();
    assert_eq!(limited["findings"].as_array().unwrap().len(), 1);
    assert!(limited["omitted"].as_u64().unwrap() >= 1);

    fs::write(dir.path().join("src/lib.rs"), "fn (((\n").unwrap();
    let broken = client.tool("check", json!({})).unwrap();
    assert_eq!(broken["status"], "incomplete", "{broken}");

    let bad = client.tool("check", json!({ "rules": ["nope/nope"] }));
    assert!(bad.is_err());
}

#[test]
fn a_verdict_recorded_through_mcp_is_an_agent_review_that_later_checks_honor() {
    let dir = rust_project();
    let mut client = Client::start(dir.path());
    client.tool("check", json!({})).unwrap();
    let tasks = client.tool("review_tasks", json!({})).unwrap();
    let task = &tasks["tasks"][0];
    assert_eq!(task["rule"], "design/private-helper-callers", "{tasks}");
    let fingerprint = task["fingerprint"].as_str().unwrap();
    let seen = task["lastSeen"].as_str().unwrap();

    let stale = client.tool(
        "review_resolve",
        json!({ "fingerprint": fingerprint, "verdict": "rejected", "reason": "intentional-exception",
                "seen": "1999-01-01T00:00:00Z" }),
    );
    assert!(stale.is_err());
    let no_reason = client.tool(
        "review_resolve",
        json!({ "fingerprint": fingerprint, "verdict": "rejected" }),
    );
    assert!(no_reason.is_err());

    let done = client
        .tool(
            "review_resolve",
            json!({ "fingerprint": fingerprint, "verdict": "rejected",
                    "reason": "intentional-exception", "note": "named policy", "seen": seen }),
        )
        .unwrap();
    assert_eq!(done["recorded"]["reviewer"], "agent:test-agent");
    assert_eq!(done["standing"], "suppressed");

    let history = client
        .tool("review_history", json!({ "fingerprint": fingerprint }))
        .unwrap();
    let event = &history["events"][0];
    assert_eq!(event["reviewerKind"], "agent");
    assert_eq!(event["reasonText"], "named policy");

    let after = client.tool("check", json!({})).unwrap();
    let still: Vec<&str> = after["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["rule"].as_str().unwrap())
        .collect();
    assert!(
        !still.contains(&"design/private-helper-callers"),
        "{still:?}"
    );
    assert_eq!(after["summary"]["suppressed"], 1);
}

#[test]
fn decision_create_writes_a_tested_decision_and_rolls_back_a_bad_one() {
    let dir = text_project();
    let mut client = Client::start(dir.path());
    let rules_dir = dir.path().join(".lighthouse/decisions");

    let created = client
        .tool(
            "decision_create",
            json!({ "id": "local/short-notes", "spec": LONG_FILE_SPEC,
                    "examples": examples("a\nb\nc\nd\n") }),
        )
        .unwrap();
    assert_eq!(created["id"], "local/short-notes");
    assert!(rules_dir.join("short-notes.yaml").is_file());
    assert_eq!(created["localPluginListed"], false);

    let tested = client
        .tool("decision_test", json!({ "ids": ["local/short-notes"] }))
        .unwrap();
    assert_eq!(tested["ok"], true, "{tested}");

    let again = client.tool(
        "decision_create",
        json!({ "id": "local/short-notes", "spec": LONG_FILE_SPEC, "examples": examples("a\nb\nc\nd\n") }),
    );
    assert!(again.unwrap_err().contains("already exists"));

    // The invalid example is too short for the rule to flag: rejected, and the
    // project keeps only the first rule.
    let rejected = client.tool(
        "decision_create",
        json!({ "id": "local/tiny-notes", "spec": LONG_FILE_SPEC, "examples": examples("a\n") }),
    );
    let message = rejected.unwrap_err();
    assert!(message.contains("not written"), "{message}");
    assert!(!rules_dir.join("tiny-notes.yaml").exists());

    let broken_rule = client.tool(
        "decision_create",
        json!({ "id": "local/tiny-notes",
                "spec": LONG_FILE_SPEC.replace("file.lines > 3", "file.nope >"),
                "examples": examples("a\nb\nc\nd\n") }),
    );
    assert!(broken_rule.is_err());
    assert!(!rules_dir.join("tiny-notes.yaml").exists());
    let leftovers: Vec<_> = fs::read_dir(&rules_dir).unwrap().collect();
    assert_eq!(leftovers.len(), 1);

    // An update goes through the same gate.
    let bad_update = client.tool(
        "decision_update",
        json!({ "id": "local/short-notes", "patch": { "check": { "where": "file.lines > 100" } } }),
    );
    assert!(bad_update.is_err());
    let before = fs::read_to_string(rules_dir.join("short-notes.yaml")).unwrap();
    assert!(before.contains("file.lines > 3"));
    let updated = client
        .tool(
            "decision_update",
            json!({ "id": "local/short-notes", "patch": { "intent": "Notes fit one screen." } }),
        )
        .unwrap();
    assert_eq!(updated["created"], false);
    assert!(
        fs::read_to_string(rules_dir.join("short-notes.yaml"))
            .unwrap()
            .contains("one screen")
    );

    // A bundled pattern is adjusted through the overlay file.
    let overlay = client
        .tool(
            "decision_update",
            json!({ "id": "core/max-file-lines", "patch": { "exceptions": "Generated files." } }),
        )
        .unwrap();
    assert_eq!(overlay["created"], true);
    assert!(
        rules_dir
            .join("override-core-max-file-lines.yaml")
            .is_file()
    );
}

#[test]
fn decision_list_and_explain_describe_decisions() {
    let dir = text_project();
    let mut client = Client::start(dir.path());
    let list = client.tool("decision_list", json!({})).unwrap();
    let row = list["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "core/max-file-lines")
        .unwrap();
    assert_eq!(row["enabled"], true);
    let explained = client
        .tool("explain", json!({ "id": "core/max-file-lines" }))
        .unwrap();
    assert!(explained["markdown"].as_str().unwrap().contains("Status:"));
    assert!(
        client
            .tool("explain", json!({ "id": "nope/nope" }))
            .is_err()
    );
}

#[test]
fn decision_names_cannot_leave_the_decisions_directory() {
    let dir = text_project();
    let mut client = Client::start(dir.path());
    for id in ["local/../../../evil", "local/a/b"] {
        let rejected = client.tool(
            "decision_create",
            json!({ "id": id, "spec": LONG_FILE_SPEC, "examples": examples("a\nb\nc\nd\n") }),
        );
        assert!(rejected.unwrap_err().contains("not a valid decision name"));
    }
    let escaped = dir.path().parent().unwrap().join("evil.yaml");
    assert!(!escaped.exists());
    assert!(!dir.path().join(".lighthouse/decisions").exists());
    let update = client.tool(
        "decision_update",
        json!({ "id": "local/../x", "patch": { "intent": "x" } }),
    );
    assert!(update.is_err());
}

#[test]
fn check_paths_stay_inside_the_project_and_diff_cannot_be_an_option() {
    let dir = rust_project();
    let mut client = Client::start(dir.path());
    let outside = client.tool("check", json!({ "paths": ["/etc"] }));
    assert!(outside.unwrap_err().contains("outside the project root"));
    let missing = client.tool("check", json!({ "paths": ["nope.rs"] }));
    assert!(missing.is_err());
    let inside = client.tool("check", json!({ "paths": ["src"] })).unwrap();
    assert_eq!(inside["status"], "findings");
    let option = client.tool("check", json!({ "diff": "--output=/tmp/lighthouse-pwned" }));
    assert!(option.unwrap_err().contains("not a git ref"));
}

const MISORDERED: &str = "pub fn run() -> u8 {\n    1\n}\n\npub struct Store;\n";
const ORDERED: &str = "pub struct Store;\n\npub fn run() -> u8 {\n    1\n}\n";

#[test]
fn fix_needs_a_selector_and_stays_inside_the_project() {
    let dir = rust_project();
    let mut client = Client::start(dir.path());

    let none = client.tool("fix", json!({})).unwrap_err();
    assert!(none.contains("name what to fix"), "{none}");

    let outside = client.tool("fix", json!({ "paths": ["/"] })).unwrap_err();
    assert!(outside.contains("outside the project root"), "{outside}");
}

#[test]
fn fix_previews_with_dry_run_then_applies_and_reports_both_lists() {
    let dir = rust_project();
    fs::write(dir.path().join("src/lib.rs"), MISORDERED).unwrap();
    let mut client = Client::start(dir.path());

    let preview = client
        .tool("fix", json!({ "paths": ["src"], "dryRun": true }))
        .unwrap();

    assert_eq!(preview["dryRun"], true);
    let diff = preview["diff"].as_str().unwrap();
    assert!(
        diff.contains("--- a/src/lib.rs") && diff.contains("+pub struct Store;"),
        "{diff}"
    );
    assert_eq!(preview["applied"][0]["rule"], "design/declaration-groups");
    assert_eq!(preview["applied"][0]["safety"], "safe");
    assert_eq!(
        fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        MISORDERED
    );

    let applied = client.tool("fix", json!({ "paths": ["src"] })).unwrap();

    assert_eq!(applied["dryRun"], false);
    assert_eq!(applied["applied"].as_array().unwrap().len(), 1);
    assert_eq!(applied["applied"][0]["files"][0], "src/lib.rs");
    assert!(applied["declined"].is_array());
    assert_eq!(
        fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        ORDERED
    );
    let after = client.tool("check", json!({})).unwrap();
    let rules: Vec<&str> = after["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["rule"].as_str())
        .collect();
    assert!(!rules.contains(&"design/declaration-groups"), "{rules:?}");
}

#[test]
fn fix_selects_by_fingerprint_and_holds_suggestions_back_until_asked() {
    let dir = rust_project();
    let banner = "// ===== Types =====\n\npub struct Store;\n";
    fs::write(dir.path().join("src/lib.rs"), banner).unwrap();
    let mut client = Client::start(dir.path());
    let found = client.tool("check", json!({})).unwrap();
    let finding = found["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule"] == "design/section-banners")
        .unwrap_or_else(|| panic!("{found}"));
    let fingerprint = finding["fingerprint"].as_str().unwrap();

    let held = client
        .tool("fix", json!({ "fingerprints": [fingerprint] }))
        .unwrap();
    assert!(held["applied"].as_array().unwrap().is_empty());
    assert!(
        held["declined"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("`unsafeFixes`")
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        banner
    );

    let done = client
        .tool(
            "fix",
            json!({ "fingerprints": [fingerprint], "unsafeFixes": true }),
        )
        .unwrap();
    assert_eq!(done["applied"][0]["safety"], "suggested");
    assert_eq!(
        fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        "pub struct Store;\n"
    );
}
