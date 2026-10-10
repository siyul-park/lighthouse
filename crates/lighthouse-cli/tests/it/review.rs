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
    let found = all.iter().find(|f| f["ruleId"] == rule).unwrap();
    found["fingerprint"].as_str().unwrap().to_owned()
}

fn rules(records: &[Value]) -> Vec<&str> {
    let mut rules: Vec<_> = records
        .iter()
        .map(|f| f["ruleId"].as_str().unwrap())
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
        .find(|f| f["ruleId"] == "design/private-helper-callers")
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
    assert!(all.iter().all(|f| f["resolvedAt"].is_string()));

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
    assert_eq!(event["reasonText"], "named policy");
    assert_eq!(event["reviewerKind"], "agent");
    assert_eq!(event["reviewerId"], "claude");
    assert_eq!(event["language"], "rust");
    assert_eq!(event["scope"], "symbol");
    assert_eq!(event["label"], "separate");
    assert_eq!(event["snapshot"]["facts"]["callers"], 1);
    assert_eq!(event["snapshot"]["evidence"]["callers"], 1);
    assert_eq!(event["snapshot"]["v"], 2);
    assert_eq!(event["snapshot"]["dirty"], false);
    assert_eq!(event["ruleVersion"].as_str().unwrap().len(), 16);
    assert_eq!(event["catalogVersion"].as_str().unwrap().len(), 16);

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
fn agent_format_groups_findings_and_agent_json_carries_the_same_groups() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let text = stdout(lighthouse(dir.path()).args(["check", "--format", "agent"]));
    for expected in [
        "design/private-helper-callers info [review] \u{2014} A private helper SHOULD have at least two callers.",
        "  expected:\n    pub fn run(x: u8) -> u8 {",
        "  evidence: caller=demo::run#function callers=1 statements=3\n",
        "  src/lib.rs:5:1 private function clamp has one caller (run); review whether",
        "resolve: lighthouse review resolve <fingerprint> ",
        "summary: 0 error, 1 warn, 1 info, 2 review, 0 incomplete, 0 suppressed",
    ] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
    assert!(
        !text.contains("fingerprint:") && !text.contains("symbol="),
        "{text}"
    );

    let json = records(&stdout(lighthouse(dir.path()).args([
        "check",
        "--format",
        "agent-json",
    ])));
    assert_eq!(json.len(), 1, "one object");
    let groups = json[0]["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(json[0]["counts"]["review"], 2);
    assert_eq!(json[0]["status"], "findings");
    assert!(json[0]["resolve"].is_string());
    let helper = group(&json[0], "design/private-helper-callers");
    assert_eq!(helper["review"], true);
    let instance = &helper["files"]["src/lib.rs"][0];
    assert_eq!(instance[0], "5:1");
    assert_eq!(instance[2].as_str().unwrap().len(), 7);
}

#[test]
fn full_detail_keeps_one_record_per_finding() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let text =
        stdout(lighthouse(dir.path()).args(["check", "--format", "agent", "--detail", "full"]));
    for expected in [
        "design/private-helper-callers  info  src/lib.rs:5:1",
        "  owner:       demo::clamp#function",
        "  requirement: A private helper SHOULD have at least two callers.",
        "  evidence:    caller=demo::run#function callers=1 statements=3",
        "  expected:    valid rust example `rust-valid` (src/lib.rs)",
        "lighthouse review resolve ",
    ] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
    let json = records(&stdout(lighthouse(dir.path()).args([
        "check",
        "--format",
        "agent-json",
        "--detail",
        "full",
    ])));
    let findings: Vec<_> = json.iter().filter(|r| r["type"] == "finding").collect();
    assert_eq!(findings.len(), 2);
    assert_eq!(json.last().unwrap()["type"], "summary");
    lighthouse(dir.path())
        .args(["check", "--detail", "full"])
        .assert()
        .code(2)
        .stderr("lighthouse: --detail applies to the agent formats only\n");
}

/// The group of `rule` in a compact report.
fn group<'a>(report: &'a Value, rule: &str) -> &'a Value {
    report["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["rule"] == rule)
        .unwrap_or_else(|| panic!("{rule} in {report}"))
}

#[test]
fn the_resolve_command_of_an_agent_finding_runs_as_given() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let text = stdout(lighthouse(dir.path()).args(["check", "--format", "agent"]));
    let template = text
        .lines()
        .find_map(|l| l.strip_prefix("resolve: "))
        .unwrap();
    let json = records(&stdout(lighthouse(dir.path()).args([
        "check",
        "--format",
        "agent-json",
    ])));
    let helper = group(&json[0], "design/private-helper-callers");
    let prefix = helper["files"]["src/lib.rs"][0][2].as_str().unwrap();
    let args: Vec<&str> = template
        .split_whitespace()
        .skip(1)
        .map(|a| match a {
            "<fingerprint>" => prefix,
            "<verdict>" => "confirmed",
            "<reason>" => "fixed",
            other => other,
        })
        .collect();
    lighthouse(dir.path()).args(args).assert().success();
    let history = records(&stdout(
        lighthouse(dir.path()).args(["review", "history", prefix, "--format", "json"]),
    ));
    assert_eq!(history[0]["reviewerKind"], "agent");
    assert_eq!(history[0]["label"], "positive");
    assert_eq!(history[0]["fingerprint"].as_str().unwrap().len(), 64);
}

#[test]
fn an_ambiguous_fingerprint_prefix_is_refused_and_a_unique_one_is_enough() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    lighthouse(dir.path()).arg("check").assert().success();
    let out = lighthouse(dir.path())
        .args(["review", "resolve", "", "--verdict", "deferred"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("matches several recorded findings"),
        "{stderr}"
    );
    let fingerprint = fingerprint_of(&dir, "design/exported-doc");
    lighthouse(dir.path())
        .args([
            "review",
            "resolve",
            &fingerprint[..7],
            "--verdict",
            "deferred",
        ])
        .assert()
        .success();
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
    let Some(plugin) = lighthouse_test_support::lang_go() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("go.mod"), "module example.com/app\n").unwrap();
    fs::write(dir.path().join("api.go"), "package app\n\nfunc Open() {}\n").unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-go\", path = {:?} }}, \"design\"]\nextends = [\"design/recommended\"]\n",
            plugin.to_str().unwrap()
        )),
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
    assert!(all[0]["resolvedAt"].is_string());
}

#[test]
fn limit_trims_agent_output_and_is_refused_for_other_formats() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    let text = stdout(lighthouse(dir.path()).args(["check", "--format", "agent", "--limit", "1"]));
    assert!(text.contains("design/exported-doc warn"), "{text}");
    assert!(!text.contains("private-helper-callers"), "{text}");
    assert!(text.contains("... 1 more finding(s) not shown"), "{text}");
    lighthouse(dir.path())
        .args(["check", "--limit", "1"])
        .assert()
        .code(2)
        .stderr("lighthouse: --limit applies to the agent formats only\n");
}

fn git_in(dir: &TempDir, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn error_level(dir: &TempDir) {
    let plugin = lighthouse_test_support::lang_rust();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"design\"]\nextends = [\"design/recommended\", \"design/strict\"]\n[rules]\n\"design/exported-doc\" = \"error\"\n",
            plugin.to_str().unwrap()
        )),
    )
    .unwrap();
}

/// The findings of any tier, mechanical included, in a status.
fn every(dir: &TempDir, status: &str) -> Vec<Value> {
    records(&stdout(lighthouse(dir.path()).args([
        "review", "list", "--all", "--status", status, "--format", "json",
    ])))
}

#[test]
fn a_mechanical_finding_stays_reported_whatever_the_verdict() {
    let source = "// lighthouse:allow design/exported-doc\npub fn run() {}\n";
    let dir = rust_project(&[("src/lib.rs", source)]);
    let plugin = lighthouse_test_support::lang_rust();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"design\", \"core\"]\nextends = [\"design/recommended\", \"core/recommended\"]\n",
            plugin.to_str().unwrap()
        )),
    )
    .unwrap();
    lighthouse(dir.path()).arg("check").assert().code(1);
    let fingerprint = every(&dir, "all")
        .iter()
        .find(|f| f["ruleId"] == "core/allow-reason")
        .map(|f| f["fingerprint"].as_str().unwrap().to_owned())
        .unwrap();
    let out = lighthouse(dir.path())
        .args([
            "review",
            "resolve",
            &fingerprint,
            "--verdict",
            "rejected",
            "--reason",
            "intentional-exception",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("mechanical finding")
    );
    let check = lighthouse(dir.path())
        .args(["check", "--format", "agent"])
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(1));
    let text = String::from_utf8(check.stdout).unwrap();
    assert!(
        text.contains("(note: rejected as intentional-exception \u{2014} mechanical findings are not suppressible; fix the rule)"),
        "{text}"
    );
    let notice = String::from_utf8(check.stderr).unwrap();
    assert!(
        notice.contains("mechanical findings are not suppressible"),
        "{notice}"
    );
    assert!(every(&dir, "suppressed").is_empty());
    assert!(
        every(&dir, "open")
            .iter()
            .any(|f| f["fingerprint"] == fingerprint.as_str())
    );
}

#[test]
fn a_heuristic_finding_configured_as_error_is_suppressed_by_a_verdict() {
    let dir = rust_project(&[("src/lib.rs", "pub fn run() {}\n")]);
    error_level(&dir);
    lighthouse(dir.path()).arg("check").assert().code(1);
    let fingerprint = fingerprint_of(&dir, "design/exported-doc");
    lighthouse(dir.path())
        .args(["review", "resolve", &fingerprint])
        .args(["--verdict", "rejected", "--reason", "false-positive"])
        .assert()
        .success();
    lighthouse(dir.path()).arg("check").assert().success();
    assert_eq!(listing(&dir, "suppressed").len(), 1);
}

#[test]
fn a_verdict_expires_when_the_evidence_it_judged_changes() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
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
    let quiet = stdout(lighthouse(dir.path()).arg("check"));
    assert!(!quiet.contains("private-helper-callers"), "{quiet}");

    let grown = HELPER.replace(
        "if x > 10 { 10 } else { x }",
        "let y = x;\n    if y > 10 { 10 } else { y }",
    );
    write(&dir, "src/lib.rs", &grown);
    let out = lighthouse(dir.path())
        .args(["check", "--format", "agent"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("private-helper-callers"), "{text}");
    assert!(
        text.contains("(note: verdict expired: evidence changed)"),
        "{text}"
    );
    let notice = String::from_utf8(out.stderr).unwrap();
    assert!(
        notice.contains("verdict expired: evidence changed"),
        "{notice}"
    );
}

#[test]
fn the_reviewer_comes_from_flags_then_the_environment_then_a_human() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    lighthouse(dir.path()).arg("check").assert().success();
    let fingerprint = fingerprint_of(&dir, "design/private-helper-callers");
    let resolve = |envs: &[(&str, &str)], flags: &[&str]| {
        let mut cmd = lighthouse(dir.path());
        cmd.env_remove("LIGHTHOUSE_REVIEWER_KIND")
            .env_remove("LIGHTHOUSE_REVIEWER")
            .env("USER", "dana");
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.args(["review", "resolve", &fingerprint, "--verdict", "deferred"])
            .args(flags)
            .assert()
            .success();
    };
    resolve(&[], &[]);
    resolve(
        &[
            ("LIGHTHOUSE_REVIEWER_KIND", "agent"),
            ("LIGHTHOUSE_REVIEWER", "hook"),
        ],
        &[],
    );
    resolve(
        &[
            ("LIGHTHOUSE_REVIEWER_KIND", "agent"),
            ("LIGHTHOUSE_REVIEWER", "hook"),
        ],
        &["--reviewer-kind", "human", "--reviewer-id", "ana"],
    );
    let history = records(&stdout(lighthouse(dir.path()).args([
        "review",
        "history",
        &fingerprint,
        "--format",
        "json",
    ])));
    let who: Vec<_> = history
        .iter()
        .map(|e| {
            (
                e["reviewerKind"].as_str().unwrap(),
                e["reviewerId"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(who.len(), 3);
    assert!(who.contains(&("human", "dana")), "{who:?}");
    assert!(who.contains(&("agent", "hook")), "{who:?}");
    assert!(who.contains(&("human", "ana")), "{who:?}");

    lighthouse(dir.path())
        .env("LIGHTHOUSE_REVIEWER_KIND", "robot")
        .args(["review", "resolve", &fingerprint, "--verdict", "deferred"])
        .assert()
        .code(2);
}

#[test]
fn the_decision_log_carries_a_verdict_to_a_clone_without_the_cache() {
    let author = rust_project(&[("src/lib.rs", HELPER)]);
    lighthouse(author.path()).arg("check").assert().success();
    let fingerprint = fingerprint_of(&author, "design/private-helper-callers");
    lighthouse(author.path())
        .args([
            "review",
            "resolve",
            &fingerprint,
            "--verdict",
            "rejected",
            "--reason",
            "project-allowed",
        ])
        .assert()
        .success();
    let log = fs::read_to_string(author.path().join(".lighthouse/decisions.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 1);

    let clone = rust_project(&[("src/lib.rs", HELPER)]);
    fs::create_dir_all(clone.path().join(".lighthouse")).unwrap();
    fs::write(clone.path().join(".lighthouse/decisions.jsonl"), &log).unwrap();
    let out = lighthouse(clone.path()).arg("check").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains("private-helper-callers"), "{text}");
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("1 finding(s) suppressed")
    );
    let history = records(&stdout(lighthouse(clone.path()).args([
        "review",
        "history",
        &fingerprint,
        "--format",
        "json",
    ])));
    assert_eq!(history[0]["reason"], "project-allowed");
}

#[test]
fn resolve_refuses_a_finding_that_was_seen_again_and_warns_about_a_resolved_one() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    lighthouse(dir.path()).arg("check").assert().success();
    let fingerprint = fingerprint_of(&dir, "design/private-helper-callers");
    lighthouse(dir.path())
        .args([
            "review",
            "resolve",
            &fingerprint,
            "--verdict",
            "deferred",
            "--seen",
            "2000-01-01T00:00:00.000Z",
        ])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("read it again before judging"));

    write(&dir, "src/lib.rs", DOCUMENTED);
    lighthouse(dir.path()).arg("check").assert().success();
    let out = lighthouse(dir.path())
        .args([
            "review",
            "resolve",
            &fingerprint,
            "--verdict",
            "confirmed",
            "--reason",
            "fixed",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("warning: the finding was already resolved")
    );
}

#[test]
fn history_says_when_nothing_was_reviewed_and_resolve_survives_a_broken_catalog() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    lighthouse(dir.path()).arg("check").assert().success();
    let fingerprint = fingerprint_of(&dir, "design/private-helper-callers");
    lighthouse(dir.path())
        .args(["review", "history", &fingerprint])
        .assert()
        .success()
        .stdout("")
        .stderr(predicates::str::contains("no reviews recorded"));

    fs::create_dir_all(dir.path().join(".lighthouse/decisions")).unwrap();
    fs::write(
        dir.path().join(".lighthouse/decisions/broken.yaml"),
        "id: [not, a, decision\n",
    )
    .unwrap();
    let out = lighthouse(dir.path())
        .args(["review", "resolve", &fingerprint, "--verdict", "deferred"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("versions are not recorded")
    );
    let history = records(&stdout(lighthouse(dir.path()).args([
        "review",
        "history",
        &fingerprint,
        "--format",
        "json",
    ])));
    assert!(history[0].get("ruleVersion").is_none());
}

#[test]
fn findings_of_a_rule_taken_out_of_the_config_are_inactive_and_can_be_pruned() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    lighthouse(dir.path()).arg("check").assert().success();
    assert_eq!(listing(&dir, "open").len(), 2);
    let plugin = lighthouse_test_support::lang_rust();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"design\"]\nextends = [\"design/recommended\"]\n",
            plugin.to_str().unwrap()
        )),
    )
    .unwrap();
    lighthouse(dir.path()).arg("check").assert().success();
    let inactive = listing(&dir, "inactive");
    assert_eq!(rules(&inactive), ["design/private-helper-callers"]);
    assert_eq!(rules(&listing(&dir, "open")), ["design/exported-doc"]);
    let out = stdout(lighthouse(dir.path()).args(["review", "list", "--status", "inactive"]));
    assert!(out.contains("\tinactive\t"), "{out}");

    let pruned = stdout(lighthouse(dir.path()).args(["review", "prune", "--older-than", "30"]));
    assert!(pruned.contains("removed 0"), "{pruned}");
    let pruned = stdout(lighthouse(dir.path()).args(["review", "prune"]));
    assert!(pruned.contains("removed 1"), "{pruned}");
    assert!(listing(&dir, "inactive").is_empty());
}

#[test]
fn changed_resolves_the_findings_of_a_deleted_file() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project("plugins = [\"core\"]\n[rules]\n\"core/max-lines\" = { level = \"warn\", options = { max = 5 } }\n"),
    )
    .unwrap();
    fs::write(dir.path().join("big.txt"), "a\nb\nc\nd\ne\nf\n").unwrap();
    fs::write(dir.path().join(".gitignore"), ".lighthouse/*.db*\n").unwrap();
    git_in(&dir, &["init", "-q", "-b", "main"]);
    git_in(&dir, &["add", "."]);
    git_in(&dir, &["commit", "-q", "-m", "base"]);
    lighthouse(dir.path()).arg("check").assert().success();
    assert_eq!(listing(&dir, "open").len(), 1);

    fs::remove_file(dir.path().join("big.txt")).unwrap();
    lighthouse(dir.path())
        .args(["check", "--changed"])
        .assert()
        .success();
    assert!(listing(&dir, "open").is_empty());
    assert_eq!(listing(&dir, "resolved").len(), 1);
}

#[test]
fn source_annotations_allow_findings_and_are_counted() {
    let allowed =
        "// lighthouse:allow design/exported-doc -- documented at its origin\npub fn run() {}\n";
    let dir = rust_project(&[("src/lib.rs", allowed)]);
    let out = lighthouse(dir.path())
        .args(["check", "--strict"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "");
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("1 finding(s) allowed by source annotations")
    );
    let agent = stdout(lighthouse(dir.path()).args(["check", "--format", "agent"]));
    assert!(agent.contains("0 suppressed, 1 allowed"), "{agent}");

    let plugin = lighthouse_test_support::lang_rust();
    fs::write(
        dir.path().join("lighthouse.toml"),
        lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"design\", \"core\"]\nextends = [\"core/recommended\", \"design/recommended\", \"design/strict\"]\n",
            plugin.to_str().unwrap()
        )),
    )
    .unwrap();
    let stale = "// lighthouse:allow design/exported-doc -- stale\n/// Runs.\npub fn run() {}\n";
    write(&dir, "src/lib.rs", stale);
    let text = stdout(lighthouse(dir.path()).arg("check"));
    assert!(text.contains("warn core/no-unused-allow"), "{text}");
}
