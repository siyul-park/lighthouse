//! Judgments and suppressions through the binary: every kind of judgment and a
//! directive in the code hide and show the same findings in a run that records,
//! a second run, a run that reads the log alone, and SARIF.

use std::fs;

use serde_json::Value;
use tempfile::TempDir;

use crate::review::{HELPER, fingerprint_of, lighthouse, listing, records, rust_project, stdout};

/// The findings a full-detail JSON report carries.
fn findings_of(out: &str) -> Vec<Value> {
    records(out)[0]["findings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

const SEVERAL: &str = "pub fn a() {}\npub fn b() {}\npub fn c() {}\npub fn d() {}\n\n// lighthouse-disable-next-line design/exported-doc -- documented at its origin\npub fn e() {}\n";

fn reported(dir: &TempDir, extra: &[&str]) -> Vec<String> {
    let out = stdout(
        lighthouse(dir.path())
            .args([
                "check",
                "--format",
                "json",
                "--detail",
                "full",
                "--decision",
                "design/exported-doc",
            ])
            .args(extra),
    );
    let mut names: Vec<String> = findings_of(&out)
        .iter()
        .filter_map(|f| f["message"].as_str())
        .map(str::to_owned)
        .collect();
    names.sort();
    names
}

fn finding_named(dir: &TempDir, name: &str) -> String {
    let out = stdout(lighthouse(dir.path()).args([
        "check",
        "--format",
        "json",
        "--detail",
        "full",
        "--decision",
        "design/exported-doc",
        "--no-store",
    ]));
    findings_of(&out)
        .iter()
        .find(|f| {
            f["message"]
                .as_str()
                .unwrap()
                .contains(&format!("function {name} "))
        })
        .unwrap_or_else(|| panic!("no finding about {name}: {out}"))["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn resolve_with(dir: &TempDir, name: &str, args: &[&str]) {
    lighthouse(dir.path())
        .args(["review", "resolve", &finding_named(dir, name)])
        .args(args)
        .assert()
        .success();
}

/// Every kind of judgment, and a directive in the code, through a run that
/// records, a second run, a run that reads the log alone, and SARIF.
#[test]
fn judgments_and_suppressions_hide_and_show_findings_the_same_in_every_run() {
    let dir = rust_project(&[("src/lib.rs", SEVERAL)]);
    assert_eq!(
        reported(&dir, &["--no-store"]).len(),
        4,
        "the directive hides e from the start"
    );
    lighthouse(dir.path()).arg("check").assert().success();

    resolve_with(&dir, "a", &["--judgment", "pass"]);
    resolve_with(
        &dir,
        "b",
        &["--judgment", "fail", "--reason", "needs a doc"],
    );
    resolve_with(
        &dir,
        "c",
        &["--judgment", "fail", "--suppress", "named policy"],
    );
    resolve_with(&dir, "d", &["--judgment", "notApplicable"]);

    let shown = reported(&dir, &[]);
    assert_eq!(
        shown.len(),
        1,
        "only the confirmed finding stays: {shown:?}"
    );
    assert!(shown[0].contains("function b "), "{shown:?}");
    assert_eq!(
        reported(&dir, &[]),
        shown,
        "and nothing expires on a second run"
    );
    assert_eq!(
        reported(&dir, &["--no-store"]).len(),
        4,
        "a run without the store applies no judgment"
    );
    assert_eq!(listing(&dir, "suppressed").len(), 3);
    assert!(
        listing(&dir, "open").is_empty(),
        "the confirmed finding is judged, so it is no review task"
    );
    assert_eq!(listing(&dir, "narrowing").len(), 1);

    let sarif: Value = serde_json::from_str(&stdout(lighthouse(dir.path()).args([
        "check",
        "--format",
        "sarif",
        "--decision",
        "design/exported-doc",
    ])))
    .unwrap();
    let results = sarif["runs"][0]["results"].as_array().unwrap();
    let suppressions = |needle: &str| -> Vec<Value> {
        results
            .iter()
            .find(|r| r["message"]["text"].as_str().unwrap().contains(needle))
            .map(|r| r["suppressions"].as_array().cloned().unwrap_or_default())
            .unwrap_or_else(|| panic!("no result about {needle}"))
    };
    assert!(
        suppressions("function b ").is_empty(),
        "a confirmed finding is just a finding"
    );
    assert_eq!(
        suppressions("function c "),
        [serde_json::json!({
            "kind": "external", "justification": "named policy"
        })]
    );
    assert_eq!(
        suppressions("function e "),
        [serde_json::json!({
            "kind": "inSource", "justification": "documented at its origin"
        })]
    );
    assert_eq!(
        results.len(),
        3,
        "a pass and a notApplicable are not SARIF results"
    );

    // A clone with the committed log and no cache reaches the same standings.
    let log = fs::read_to_string(dir.path().join(".lighthouse/decisions.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 5, "four judgments and one suppression");
    let clone = rust_project(&[("src/lib.rs", SEVERAL)]);
    fs::create_dir_all(clone.path().join(".lighthouse")).unwrap();
    fs::write(clone.path().join(".lighthouse/decisions.jsonl"), &log).unwrap();
    assert_eq!(reported(&clone, &[]), shown);
    assert_eq!(listing(&clone, "suppressed").len(), 3);
}

#[test]
fn a_suppression_without_a_justification_is_refused_and_records_nothing() {
    let dir = rust_project(&[("src/lib.rs", SEVERAL)]);
    lighthouse(dir.path()).arg("check").assert().success();
    let fingerprint = finding_named(&dir, "a");

    for blank in ["", "   "] {
        lighthouse(dir.path())
            .args(["review", "resolve", &fingerprint])
            .args(["--judgment", "fail", "--suppress", blank])
            .assert()
            .code(2)
            .stderr(predicates::str::contains("needs a justification"));
    }

    assert!(!dir.path().join(".lighthouse/decisions.jsonl").exists());
}

#[test]
fn a_short_fingerprint_prefix_is_refused_and_a_unique_one_is_enough() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    lighthouse(dir.path()).arg("check").assert().success();
    let out = lighthouse(dir.path())
        .args(["review", "resolve", "abc", "--judgment", "fail"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("give at least 7 characters"), "{stderr}");
    let fingerprint = fingerprint_of(&dir, "design/exported-doc");
    lighthouse(dir.path())
        .args(["review", "resolve", &fingerprint[..7], "--judgment", "fail"])
        .assert()
        .success();
}

#[test]
fn review_show_and_text_listings_describe_a_finding() {
    let dir = rust_project(&[("src/lib.rs", HELPER)]);
    assert_eq!(stdout(lighthouse(dir.path()).args(["review", "list"])), "");
    lighthouse(dir.path()).arg("check").assert().success();
    let fingerprint = fingerprint_of(&dir, "design/no-panic");
    let list =
        stdout(lighthouse(dir.path()).args(["review", "list", "--decision", "design/no-panic"]));
    assert_eq!(list.lines().count(), 1);
    assert!(list.starts_with(&fingerprint[..12]), "{list}");
    assert!(list.contains("\topen\t-\t"), "{list}");

    let show = stdout(lighthouse(dir.path()).args(["review", "show", &fingerprint[..10]]));
    for expected in [
        "owner:       demo::run#function",
        "state:       open",
        "location:    src/lib.rs:2",
        "kind=\"unwrap\"",
    ] {
        assert!(show.contains(expected), "{expected}\n{show}");
    }
    lighthouse(dir.path())
        .args(["review", "show", "nonesuch"])
        .assert()
        .code(2);
}
