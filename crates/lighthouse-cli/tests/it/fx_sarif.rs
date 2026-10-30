//! The fixes of a check in SARIF.

use serde_json::Value;
use tempfile::TempDir;

use crate::{
    fix::{lighthouse, read},
    fx::{LONG, project},
};

/// The `results` of the SARIF of `lighthouse check` with `args` over the
/// decisions that are fixed by an ops fix and by the agent.
fn sarif_results(dir: &TempDir, args: &[&str]) -> Value {
    let out = lighthouse(dir.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(["check", "--no-store", "--format", "sarif"])
        .args([
            "--decision",
            "local/ops-unsafe",
            "--decision",
            "local/model",
        ])
        .args(args)
        .output()
        .unwrap();
    let sarif: Value = serde_json::from_slice(&out.stdout).unwrap();
    sarif["runs"][0]["results"].clone()
}

#[test]
fn a_dry_run_in_sarif_carries_every_verified_fix_as_fixes() {
    let (dir, _agent) = project("");

    let results = sarif_results(&dir, &["--fix", "--dry-run"]);

    insta::assert_snapshot!(serde_json::to_string_pretty(&results).unwrap());
    assert_eq!(read(&dir, "model.txt"), LONG, "nothing was written");
}

#[test]
fn a_fix_run_in_sarif_carries_the_fixes_it_left_as_previews_only() {
    let (dir, _agent) = project("");

    let results = sarif_results(&dir, &["--fix"]);

    let fixed = |rule: &str| {
        results
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["ruleId"] == rule)
            .all(|r| r.get("fixes").is_some())
    };
    assert!(
        fixed("local/ops-unsafe") && fixed("local/model"),
        "{results}"
    );
    let (safe, _agent) = project("");
    let applied = lighthouse(safe.path())
        .env("LIGHTHOUSE_TRUST", "1")
        .args(["check", "--no-store", "--format", "sarif", "--fix"])
        .args(["--decision", "local/ops-safe"])
        .output()
        .unwrap();
    let sarif: Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert!(
        !sarif.to_string().contains("\"fixes\""),
        "a written fix has no fixes: {sarif}"
    );
}

#[test]
fn a_check_in_sarif_without_fix_carries_no_fixes() {
    let (dir, _agent) = project("");

    let results = sarif_results(&dir, &[]);

    assert!(!results.to_string().contains("\"fixes\""), "{results}");
}
