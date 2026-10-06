//! The remembering loop through the binary: `check` records findings, `review`
//! judges them, and later checks honor the verdicts.

use std::{fs, path::Path};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const HELPER: &str = "pub fn run(x: u8) -> u8 {\n    clamp(x) + 1\n}\n\nfn clamp(x: u8) -> u8 {\n    if x > 10 { 10 } else { x }\n}\n";
const DOCUMENTED: &str = "/// Runs.\npub fn run(x: u8) -> u8 {\n    x + 1\n}\n";

fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir);
    cmd
}

/// A one-crate Rust project whose config enables the design rules, review
/// advice included.
fn rust_project(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let plugin = lighthouse_testkit::lang_rust();
    fs::write(
        dir.path().join("lighthouse.toml"),
        format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"design\"]\nextends = [\"design/recommended\", \"design/strict\"]\n",
            plugin.to_str().unwrap()
        ),
    )
    .unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    for (path, text) in files {
        fs::write(dir.path().join(path), text).unwrap();
    }
    dir
}

fn write(dir: &TempDir, path: &str, text: &str) {
    fs::write(dir.path().join(path), text).unwrap();
}

fn stdout(command: &mut Command) -> String {
    String::from_utf8(command.output().unwrap().stdout).unwrap()
}

fn records(text: &str) -> Vec<Value> {
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn listing(dir: &TempDir, status: &str) -> Vec<Value> {
    let out = stdout(
        lighthouse(dir.path()).args(["review", "list", "--status", status, "--format", "json"]),
    );
    records(&out)
}

fn fingerprint_of(dir: &TempDir, rule: &str) -> String {
    let all = listing(dir, "all");
    let found = all.iter().find(|f| f["rule_id"] == rule).unwrap();
    found["fingerprint"].as_str().unwrap().to_owned()
}

fn rules(records: &[Value]) -> Vec<&str> {
    let mut rules: Vec<_> = records
        .iter()
        .map(|f| f["rule_id"].as_str().unwrap())
        .collect();
    rules.sort_unstable();
    rules
}

#[test]
fn check_remembers_findings_and_marks_fixed_ones_resolved() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    lighthouse(dir.path()).arg("check").assert().success();
    let open = listing(&dir, "open");
    assert_eq!(
        rules(&open),
        ["design/exported-doc", "design/private-helper-callers"]
    );
    let helper = open
        .iter()
        .find(|f| f["rule_id"] == "design/private-helper-callers")
        .unwrap();
    assert_eq!(helper["symbol"], "demo::clamp#function");
    assert_eq!(helper["facts"]["language"], "rust");
    assert_eq!(helper["facts"]["callers"], 1);
    assert_eq!(helper["locator"]["span"]["start"]["line"], 5);

    write(&dir, "src/lib.rs", DOCUMENTED);
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .success()
        .stdout("");
    assert!(listing(&dir, "open").is_empty());
    let all = listing(&dir, "all");
    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|f| f["resolved_at"].is_string()));

    write(&dir, "src/lib.rs", HELPER);
    lighthouse(dir.path()).arg("check").assert().success();
    let back = listing(&dir, "open");
    assert_eq!(back.len(), 2);
    assert!(back.iter().all(|f| f["reopened"] == 1));
}

#[test]
fn a_rejected_verdict_keeps_the_finding_out_of_later_reports() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    lighthouse(dir.path()).arg("check").assert().success();
    let fingerprint = fingerprint_of(&dir, "design/private-helper-callers");

    lighthouse(dir.path())
        .args(["review", "resolve", &fingerprint[..12]])
        .args(["--verdict", "rejected", "--reason", "intentional-exception"])
        .args([
            "--note",
            "named policy",
            "--reviewer-kind",
            "agent",
            "--reviewer-id",
            "claude",
        ])
        .assert()
        .success();

    let out = lighthouse(dir.path()).arg("check").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains("private-helper-callers"), "{text}");
    assert!(text.contains("design/exported-doc"), "{text}");
    let notice = String::from_utf8(out.stderr).unwrap();
    assert!(
        notice.contains("1 finding(s) suppressed by review verdicts"),
        "{notice}"
    );
    let agent = stdout(lighthouse(dir.path()).args(["check", "--format", "agent"]));
    assert!(agent.contains("0 incomplete, 1 suppressed"), "{agent}");

    assert_eq!(
        rules(&listing(&dir, "suppressed")),
        ["design/private-helper-callers"]
    );
    assert_eq!(rules(&listing(&dir, "open")), ["design/exported-doc"]);

    let history = records(&stdout(lighthouse(dir.path()).args([
        "review",
        "history",
        &fingerprint,
        "--format",
        "json",
    ])));
    assert_eq!(history.len(), 1);
    let event = &history[0];
    assert_eq!(event["verdict"], "rejected");
    assert_eq!(event["reason"], "intentional-exception");
    assert_eq!(event["reason_text"], "named policy");
    assert_eq!(event["reviewer_kind"], "agent");
    assert_eq!(event["reviewer_id"], "claude");
    assert_eq!(event["language"], "rust");
    assert_eq!(event["scope"], "symbol");
    assert_eq!(event["label"], "separate");
    assert_eq!(event["feature_snapshot"]["facts"]["callers"], 1);
    assert_eq!(event["evidence"]["callers"], 1);
    assert_eq!(event["rule_version"].as_str().unwrap().len(), 16);
    assert_eq!(event["catalog_version"].as_str().unwrap().len(), 16);

    lighthouse(dir.path())
        .args(["review", "resolve", &fingerprint, "--verdict", "deferred"])
        .assert()
        .success();
    let again = stdout(lighthouse(dir.path()).arg("check"));
    assert!(again.contains("private-helper-callers"), "{again}");
}

#[test]
fn a_suppressed_error_no_longer_fails_the_run() {
    let dir = rust_project(&[("src/lib.rs", DOCUMENTED)]);
    let strict = |dir: &TempDir| lighthouse(dir.path()).args(["check", "--strict"]).assert();
    write(&dir, "src/lib.rs", "pub fn run() {}\n");
    strict(&dir).code(1);
    let fingerprint = fingerprint_of(&dir, "design/exported-doc");
    lighthouse(dir.path())
        .args(["review", "resolve", &fingerprint])
        .args(["--verdict", "rejected", "--reason", "project-allowed"])
        .assert()
        .success();
    strict(&dir).success();
}

#[test]
fn resolve_rejects_inconsistent_verdicts_and_unknown_findings() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    lighthouse(dir.path())
        .args(["review", "resolve", "abc", "--verdict", "deferred"])
        .assert()
        .code(2)
        .stderr("lighthouse: no findings recorded yet (run `lighthouse check`)\n");
    lighthouse(dir.path()).arg("check").assert().success();
    let fingerprint = fingerprint_of(&dir, "design/exported-doc");

    let fails = |args: &[&str], message: &str| {
        let out = lighthouse(dir.path())
            .args(["review", "resolve", &fingerprint])
            .args(args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(stderr.contains(message), "{stderr}");
    };
    fails(
        &["--verdict", "rejected"],
        "does not fit verdict `rejected`",
    );
    fails(
        &["--verdict", "confirmed", "--reason", "false-positive"],
        "does not fit verdict",
    );
    fails(
        &["--verdict", "deferred", "--reason", "fixed"],
        "does not fit verdict",
    );
    fails(&["--verdict", "maybe"], "unknown verdict");
    fails(
        &["--verdict", "confirmed", "--reviewer-kind", "robot"],
        "unknown reviewer kind",
    );
    lighthouse(dir.path())
        .args(["review", "resolve", "ffffffff", "--verdict", "deferred"])
        .assert()
        .code(2);
    assert!(
        records(&stdout(lighthouse(dir.path()).args([
            "review",
            "history",
            &fingerprint,
            "--format",
            "json"
        ])))
        .is_empty()
    );
}

#[test]
fn an_incomplete_run_never_resolves_findings() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    lighthouse(dir.path()).arg("check").assert().success();
    write(&dir, "src/lib.rs", &format!("{DOCUMENTED}pub mod bad;\n"));
    write(&dir, "src/bad.rs", "pub fn broken( {\n");
    lighthouse(dir.path())
        .args(["check", "--allow-incomplete"])
        .assert()
        .success();
    let open = listing(&dir, "open");
    assert_eq!(open.len(), 2, "{open:?}");

    fs::remove_file(dir.path().join("src/bad.rs")).unwrap();
    write(&dir, "src/lib.rs", DOCUMENTED);
    lighthouse(dir.path()).arg("check").assert().success();
    assert!(listing(&dir, "open").is_empty());
}

#[test]
fn a_run_resolves_only_findings_inside_its_report_scope() {
    let dir = rust_project(&[
        ("src/lib.rs", "/// Both.\npub mod a;\npub mod b;\n"),
        ("src/a.rs", "pub fn a() {}\n"),
        ("src/b.rs", "pub fn b() {}\n"),
    ]);
    lighthouse(dir.path()).arg("check").assert().success();
    assert_eq!(listing(&dir, "open").len(), 2);

    write(&dir, "src/a.rs", "/// A.\npub fn a() {}\n");
    write(&dir, "src/b.rs", "/// B.\npub fn b() {}\n");
    lighthouse(dir.path())
        .args(["check", "src/a.rs"])
        .assert()
        .success();
    let open = listing(&dir, "open");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["path"], "src/b.rs");

    lighthouse(dir.path())
        .args([
            "check",
            "--rules",
            "design/private-helper-callers",
            "src/b.rs",
        ])
        .assert()
        .success();
    assert_eq!(listing(&dir, "open").len(), 1);

    lighthouse(dir.path()).arg("check").assert().success();
    assert!(listing(&dir, "open").is_empty());
}

#[test]
fn no_store_neither_records_nor_applies_verdicts() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    lighthouse(dir.path())
        .args(["check", "--no-store"])
        .assert()
        .success();
    assert!(!dir.path().join(".lighthouse").exists());

    lighthouse(dir.path()).arg("check").assert().success();
    let fingerprint = fingerprint_of(&dir, "design/private-helper-callers");
    lighthouse(dir.path())
        .args([
            "review",
            "resolve",
            &fingerprint,
            "--verdict",
            "rejected",
            "--reason",
            "false-positive",
        ])
        .assert()
        .success();
    let out = stdout(lighthouse(dir.path()).args(["check", "--no-store"]));
    assert!(out.contains("private-helper-callers"), "{out}");
}

#[test]
fn agent_format_briefs_the_agent_and_agent_json_carries_the_same_records() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let text = stdout(lighthouse(dir.path()).args(["check", "--format", "agent"]));
    for expected in [
        "design/private-helper-callers  review (heuristic)  src/lib.rs:5:1",
        "  owner:       demo::clamp#function",
        "  requirement: A private helper SHOULD have at least two callers.",
        "  evidence:    caller=demo::run#function callers=1 statements=3",
        "  expected:    valid rust example `rust-valid` (src/lib.rs)",
        "lighthouse review resolve ",
        "summary: 0 error, 1 warn, 1 review, 0 incomplete, 0 suppressed",
    ] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }

    let json = records(&stdout(lighthouse(dir.path()).args([
        "check",
        "--format",
        "agent-json",
    ])));
    let findings: Vec<_> = json.iter().filter(|r| r["type"] == "finding").collect();
    assert_eq!(findings.len(), 2);
    let review = findings.iter().find(|f| f["severity"] == "review").unwrap();
    let command = review["resolve"]["command"].as_str().unwrap();
    assert!(command.contains(review["fingerprint"].as_str().unwrap()));
    assert_eq!(json.last().unwrap()["type"], "summary");
}

#[test]
fn the_resolve_command_of_an_agent_finding_runs_as_given() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let json = records(&stdout(lighthouse(dir.path()).args([
        "check",
        "--format",
        "agent-json",
    ])));
    let review = json.iter().find(|r| r["severity"] == "review").unwrap();
    let command = review["resolve"]["command"].as_str().unwrap();
    let args: Vec<&str> = command
        .split_whitespace()
        .skip(1)
        .map(|a| match a {
            "<verdict>" => "confirmed",
            "<reason>" => "fixed",
            other => other,
        })
        .collect();
    lighthouse(dir.path()).args(args).assert().success();
    let history = records(&stdout(lighthouse(dir.path()).args([
        "review",
        "history",
        review["fingerprint"].as_str().unwrap(),
        "--format",
        "json",
    ])));
    assert_eq!(history[0]["reviewer_kind"], "agent");
    assert_eq!(history[0]["label"], "positive");
}

#[test]
fn review_show_and_text_listings_describe_a_finding() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    assert_eq!(stdout(lighthouse(dir.path()).args(["review", "list"])), "");
    lighthouse(dir.path()).arg("check").assert().success();
    let fingerprint = fingerprint_of(&dir, "design/private-helper-callers");
    let list = stdout(lighthouse(dir.path()).args([
        "review",
        "list",
        "--rule",
        "design/private-helper-callers",
    ]));
    assert_eq!(list.lines().count(), 1);
    assert!(list.starts_with(&fingerprint[..12]), "{list}");
    assert!(list.contains("\topen\t-\t"), "{list}");

    let show = stdout(lighthouse(dir.path()).args(["review", "show", &fingerprint[..10]]));
    for expected in [
        "owner:       demo::clamp#function",
        "state:       open",
        "location:    src/lib.rs:5",
        "callers=1",
    ] {
        assert!(show.contains(expected), "{expected}\n{show}");
    }
    lighthouse(dir.path())
        .args(["review", "show", "nonesuch"])
        .assert()
        .code(2);
}

#[test]
fn go_findings_are_remembered_and_resolved_like_any_other() {
    let Some(plugin) = lighthouse_testkit::lang_go() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("go.mod"), "module example.com/app\n").unwrap();
    fs::write(dir.path().join("api.go"), "package app\n\nfunc Open() {}\n").unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        format!(
            "plugins = [{{ id = \"lang-go\", path = {:?} }}, \"design\"]\nextends = [\"design/recommended\"]\n",
            plugin.to_str().unwrap()
        ),
    )
    .unwrap();
    lighthouse(dir.path()).arg("check").assert().success();
    let open = records(&stdout(
        lighthouse(dir.path()).args(["review", "list", "--format", "json"]),
    ));
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["facts"]["language"], "go");
    assert_eq!(open[0]["symbol"], ".::Open#function");

    fs::write(
        dir.path().join("api.go"),
        "package app\n\n// Open opens.\nfunc Open() {}\n",
    )
    .unwrap();
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .success()
        .stdout("");
    let all =
        records(&stdout(lighthouse(dir.path()).args([
            "review", "list", "--status", "all", "--format", "json",
        ])));
    assert!(all[0]["resolved_at"].is_string());
}
