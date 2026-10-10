//! Decisions are identified by uid: after a project is migrated, the verdicts
//! recorded under the old names and fingerprints still apply.

use std::{fs, path::Path};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir);
    cmd
}

fn write_at(dir: &TempDir, path: &str, text: &str) {
    let path = dir.path().join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
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
    records(&stdout(lighthouse(dir.path()).args([
        "review", "list", "--status", status, "--format", "json",
    ])))
}

fn rules(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .map(|f| f["ruleId"].as_str().unwrap())
        .collect()
}

/// A local decision as an earlier build wrote it: no `uid`.
fn probe(name: &str, annotations: &str) -> String {
    format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/{name}\n{annotations}spec:\n  title: Probe\n  context: A probe.\n  scope: {{ subject: file }}\n  requirement: A file SHOULD have at most one line.\n  severity: warn\n  check:\n    type: cel\n    where: \"file.path == 'a.txt' && file.lines > 1\"\n    message: too long\n    evidence: {{ path: file.path }}\n  examples:\n    - name: long\n      language: text\n      kind: invalid\n      files: [{{ path: a.txt, body: \"1\\n2\" }}]\n      expect: [{{ line: 1 }}]\n    - name: short\n      language: text\n      kind: valid\n      files: [{{ path: a.txt, body: \"1\" }}]\n"
    )
}

fn text_project(rule: &str) -> String {
    lighthouse_test_support::project(&format!(
        "plugins = [\"core\", \"local\"]\n[rules]\n\"local/{rule}\" = \"warn\"\n"
    ))
}

fn rewrites(dir: &TempDir) -> usize {
    fs::read_to_string(dir.path().join(".lighthouse/decisions.jsonl"))
        .unwrap()
        .matches("\"kind\":\"Rewrite\"")
        .count()
}

#[test]
fn verdicts_under_old_names_and_fingerprints_still_apply_after_migration() {
    let dir = tempfile::tempdir().unwrap();
    write_at(&dir, "lighthouse.toml", &text_project("probe"));
    write_at(
        &dir,
        ".lighthouse/decisions/probe.yaml",
        &probe("probe", ""),
    );
    write_at(&dir, "a.txt", "1\n2\n");
    lighthouse(dir.path()).arg("check").assert().success();
    let old = listing(&dir, "all");
    assert_eq!(rules(&old), ["local/probe"]);
    let old_fingerprint = old[0]["fingerprint"].as_str().unwrap().to_owned();
    lighthouse(dir.path())
        .args(["review", "resolve", &old_fingerprint])
        .args(["--verdict", "rejected", "--reason", "false-positive"])
        .assert()
        .success();
    let quiet = stdout(lighthouse(dir.path()).arg("check"));
    assert_eq!(quiet, "", "the verdict applies before the migration");

    // The decision is renamed, its old name kept, and the project migrated.
    fs::remove_file(dir.path().join(".lighthouse/decisions/probe.yaml")).unwrap();
    write_at(
        &dir,
        ".lighthouse/decisions/renamed.yaml",
        &probe(
            "renamed",
            "  annotations:\n    lighthouse/was-names: local/probe\n",
        ),
    );
    write_at(&dir, "lighthouse.toml", &text_project("renamed"));
    lighthouse(dir.path())
        .args(["spec", "migrate", ".lighthouse"])
        .assert()
        .success();
    let migrated =
        fs::read_to_string(dir.path().join(".lighthouse/decisions/renamed.yaml")).unwrap();
    let uid = migrated
        .lines()
        .find_map(|l| l.trim().strip_prefix("uid: "))
        .expect("migrate assigns a uid")
        .to_owned();

    assert_eq!(rewrites(&dir), 0, "nothing moves before a run sees both");
    let out = lighthouse(dir.path()).arg("check").output().unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "");
    let notices = String::from_utf8(out.stderr).unwrap();
    assert!(
        notices.contains("1 finding(s) suppressed by review verdicts"),
        "{notices}"
    );
    assert!(!notices.contains("expired"), "{notices}");

    let all = listing(&dir, "all");
    assert_eq!(all.len(), 1, "no orphaned record: {all:?}");
    assert_eq!(all[0]["ruleId"], "local/renamed");
    assert_eq!(all[0]["decisionUid"], uid.as_str());
    assert_eq!(all[0]["standing"], "suppressed");
    assert_ne!(all[0]["fingerprint"], old_fingerprint.as_str());
    assert!(listing(&dir, "resolved").is_empty());
    assert_eq!(rewrites(&dir), 1);

    let fingerprint = all[0]["fingerprint"].as_str().unwrap();
    let history = records(&stdout(lighthouse(dir.path()).args([
        "review",
        "history",
        fingerprint,
        "--format",
        "json",
    ])));
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["ruleId"], "local/probe", "the name it had then");
    assert_eq!(history[0]["decisionUid"], uid.as_str());

    lighthouse(dir.path()).arg("check").assert().success();
    assert_eq!(rewrites(&dir), 1, "the rewrite is recorded once");
    assert_eq!(listing(&dir, "all").len(), 1);
}

#[test]
fn an_old_decision_id_still_works_with_a_notice_and_migrate_rewrites_it() {
    let dir = tempfile::tempdir().unwrap();
    write_at(
        &dir,
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\"]\n[rules]\n\"core/max-file-lines\" = { level = \"warn\", options = { max = 1 } }\n",
        ),
    );
    write_at(&dir, "a.txt", "1\n2\n");
    write_at(
        &dir,
        "src/lib.rs",
        "// lighthouse:allow core/max-file-lines, design/exported-doc -- old names\npub fn open() {}\n",
    );

    let out = lighthouse(dir.path())
        .args(["check", "--no-store"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("warn core/max-lines"), "{text}");
    let notices = String::from_utf8(out.stderr).unwrap();
    assert!(
        notices.contains("decision `core/max-file-lines` is now `core/max-lines`"),
        "{notices}"
    );
    let only = lighthouse(dir.path())
        .args(["check", "--no-store", "--rules", "core/max-file-lines"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8(only.stdout)
            .unwrap()
            .contains("core/max-lines")
    );

    lighthouse(dir.path())
        .args(["spec", "migrate", "."])
        .assert()
        .success();
    let config = fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap();
    assert!(config.contains("core/max-lines"), "{config}");
    assert!(!config.contains("max-file-lines"), "{config}");
    let source = fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        source.starts_with(
            "// lighthouse-disable-next-line core/max-lines, design/exported-doc -- old names\n"
        ),
        "{source}"
    );
    let notices = String::from_utf8(
        lighthouse(dir.path())
            .args(["check", "--no-store"])
            .output()
            .unwrap()
            .stderr,
    )
    .unwrap();
    assert!(!notices.contains("is now"), "{notices}");
}
