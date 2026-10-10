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
    let plugin = lighthouse_test_support::lang_rust();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project(&format!(
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
        lighthouse_test_support::project(
            "plugins = [\"core\"]\nextends = [\"core/recommended\"]\n",
        ),
    )
    .unwrap();
    dir
}

const LONG_FILE_SPEC: &str = r#"
title: Notes stay short
context: Notes are read in one glance.
scope: { subject: file }
requirement: A note file MUST NOT exceed three lines.
severity: error
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

/// The rules of the groups of a compact report.
fn rules_of(report: &Value) -> Vec<&str> {
    report["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["rule"].as_str().unwrap())
        .collect()
}

/// How many findings the groups of a compact report hold.
fn shown(report: &Value) -> usize {
    report["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["files"].as_object().unwrap().values())
        .map(|rows| rows.as_array().unwrap().len())
        .sum()
}

#[test]
fn check_reports_findings_and_never_calls_an_incomplete_run_clean() {
    let dir = rust_project();
    let mut client = Client::start(dir.path());
    let found = client.tool("check", json!({})).unwrap();
    assert_eq!(found["status"], "findings");
    assert!(rules_of(&found).contains(&"design/exported-doc"), "{found}");
    assert!(found["counts"]["warn"].as_u64().unwrap() >= 1);
    assert!(found.get("summary").is_none() && found.get("findings").is_none());

    let limited = client.tool("check", json!({ "limit": 1 })).unwrap();
    assert_eq!(shown(&limited), 1);
    assert!(limited["omitted"]["findings"].as_u64().unwrap() >= 1);

    let full = client.tool("check", json!({ "detail": "full" })).unwrap();
    let records = full["findings"].as_array().unwrap();
    assert!(records.iter().any(|f| f["rule"] == "design/exported-doc"));
    assert!(records[0]["location"]["line"].is_u64(), "{full}");
    assert!(full["summary"]["warnings"].as_u64().unwrap() >= 1);
    assert!(
        client
            .tool("check", json!({ "detail": "verbose" }))
            .is_err()
    );

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
    let group = tasks["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["rule"] == "design/private-helper-callers")
        .unwrap_or_else(|| panic!("{tasks}"));
    let (path, rows) = group["files"].as_object().unwrap().iter().next().unwrap();
    let instance = &rows[0];
    let prefix = instance[2].as_str().unwrap();
    assert_eq!(prefix.len(), 7, "{path} {instance}");
    let seen = group["evidence"]["seen"]
        .as_str()
        .or_else(|| instance[3]["evidence"]["seen"].as_str())
        .unwrap();
    assert!(tasks["resolve"].is_string() && tasks["reasons"].is_object());

    let full = client
        .tool("review_tasks", json!({ "detail": "full" }))
        .unwrap();
    let task = full["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["rule"] == "design/private-helper-callers")
        .unwrap();
    let fingerprint = task["fingerprint"].as_str().unwrap();
    assert!(fingerprint.starts_with(prefix));
    assert_eq!(task["lastSeen"], seen);

    let ambiguous = client
        .tool(
            "review_resolve",
            json!({ "fingerprint": "", "verdict": "deferred" }),
        )
        .unwrap_err();
    assert!(ambiguous.contains("matches several"), "{ambiguous}");
    let stale = client.tool(
        "review_resolve",
        json!({ "fingerprint": prefix, "verdict": "rejected", "reason": "intentional-exception",
                "seen": "1999-01-01T00:00:00Z" }),
    );
    assert!(stale.is_err());
    let no_reason = client.tool(
        "review_resolve",
        json!({ "fingerprint": prefix, "verdict": "rejected" }),
    );
    assert!(no_reason.is_err());

    let done = client
        .tool(
            "review_resolve",
            json!({ "fingerprint": prefix, "verdict": "rejected",
                    "reason": "intentional-exception", "note": "named policy", "seen": seen }),
        )
        .unwrap();
    assert_eq!(done["recorded"]["reviewer"], "agent:test-agent");
    assert_eq!(done["recorded"]["fingerprint"], fingerprint);
    assert_eq!(done["standing"], "suppressed");

    let history = client
        .tool("review_history", json!({ "fingerprint": prefix }))
        .unwrap();
    let event = &history["events"][0];
    assert_eq!(event["reviewerKind"], "agent");
    assert_eq!(event["reasonText"], "named policy");

    let after = client.tool("check", json!({})).unwrap();
    let still = rules_of(&after);
    assert!(
        !still.contains(&"design/private-helper-callers"),
        "{still:?}"
    );
    assert_eq!(after["counts"]["suppressed"], 1);
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
    let written = fs::read_to_string(rules_dir.join("short-notes.yaml")).unwrap();
    assert!(
        written
            .lines()
            .filter_map(|l| l.trim().strip_prefix("uid: "))
            .any(|uid| lighthouse_resource::is_uid(uid.trim_matches('"'))),
        "a new decision gets a uid: {written}"
    );
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
            json!({ "id": "local/short-notes", "patch": { "context": "Notes fit one screen." } }),
        )
        .unwrap();
    assert_eq!(updated["created"], false);
    assert!(
        fs::read_to_string(rules_dir.join("short-notes.yaml"))
            .unwrap()
            .contains("one screen")
    );

    // A bundled decision is not changed in place: its level and options are
    // the project's `rules`.
    let bundled = client.tool(
        "decision_update",
        json!({ "id": "core/max-file-lines", "patch": { "requirement": "A file MUST be short." } }),
    );
    assert!(bundled.unwrap_err().to_string().contains("rules"));
    assert!(!rules_dir.join("override-core-max-file-lines.yaml").exists());
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
        json!({ "id": "local/../x", "patch": { "context": "x" } }),
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
    let rules = rules_of(&after);
    assert!(!rules.contains(&"design/declaration-groups"), "{rules:?}");
}

#[test]
fn fix_selects_by_fingerprint_and_holds_suggestions_back_until_asked() {
    let dir = rust_project();
    let banner = "// ===== Types =====\n\npub struct Store;\n";
    fs::write(dir.path().join("src/lib.rs"), banner).unwrap();
    let mut client = Client::start(dir.path());
    let found = client.tool("check", json!({})).unwrap();
    let banners = found["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["rule"] == "design/section-banners")
        .unwrap_or_else(|| panic!("{found}"));
    let fingerprint = banners["files"]["src/lib.rs"][0][2].as_str().unwrap();

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
