//! `lighthouse spec validate`, `spec migrate` and `schema` through the binary,
//! and the schemas checked in under `schema/`.

use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;

fn lighthouse(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("lighthouse").unwrap();
    cmd.current_dir(dir);
    cmd
}

fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

const OLD_CONFIG: &str = "plugins = [\"core\"]\nextends = [\"core/recommended\"]\n[rules]\n\"core/max-file-lines\" = { level = \"review\", max = 2 }\n";

const OLD_RULE: &str = "id: local/long
title: Long
intent: Long files are hard to read.
scope: file
requirement: A file MUST have at most three lines.
enforcement: mechanical
evidence: [path]
examples:
  - name: long
    language: text
    kind: invalid
    files: [{ path: a.txt, body: \"1\\n2\\n3\\n4\" }]
    expect: [{ line: 1 }]
  - name: short
    language: text
    kind: valid
    files: [{ path: a.txt, body: \"1\" }]
rule:
  select: file
  where: 'file.lines > 3'
  message: too long
";

#[test]
fn migrate_rewrites_old_documents_once_and_validate_accepts_the_result() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lighthouse.toml", OLD_CONFIG);
    write(dir.path(), ".lighthouse/rules/long.yaml", OLD_RULE);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 file(s) written, 1 removed"));

    assert!(!dir.path().join(".lighthouse/rules/long.yaml").exists());
    let decision = fs::read_to_string(dir.path().join(".lighthouse/decisions/long.yaml")).unwrap();
    assert!(decision.contains("kind: Decision"), "{decision}");
    assert!(decision.contains("type: cel"), "{decision}");
    let config = fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap();
    assert!(config.contains("kind = \"Project\""), "{config}");
    assert!(
        config.contains("level = \"info\""),
        "review is info now: {config}"
    );

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 file(s) written, 0 removed"));
    lighthouse(dir.path())
        .args(["spec", "validate", "--examples"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 problem(s)"));
}

const OTHER_RULE: &str = "id: local/short
title: Short
intent: Short files are easy to read.
scope: file
requirement: A file MUST have at least one line.
enforcement: mechanical
rule:
  select: file
  where: 'file.lines < 1'
  message: empty
";

#[test]
fn migrate_applies_every_write_before_it_removes_anything() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".lighthouse/rules/long.yaml", OLD_RULE);
    // The target directory cannot be made: nothing may have been removed.
    write(dir.path(), ".lighthouse/decisions", "in the way");

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .failure();

    let rule = dir.path().join(".lighthouse/rules/long.yaml");
    assert_eq!(fs::read_to_string(rule).unwrap(), OLD_RULE);
}

#[test]
fn migrate_refuses_a_file_of_several_documents_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let two = format!("{OLD_RULE}---\n{OTHER_RULE}");
    write(dir.path(), ".lighthouse/rules/two.yaml", &two);
    write(dir.path(), ".lighthouse/rules/long.yaml", OLD_RULE);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("holds 2 documents"));

    assert_eq!(
        fs::read_to_string(dir.path().join(".lighthouse/rules/two.yaml")).unwrap(),
        two
    );
    assert!(dir.path().join(".lighthouse/rules/long.yaml").exists());
    assert!(!dir.path().join(".lighthouse/decisions").exists());
}

#[test]
fn migrate_moves_a_rules_directory_without_a_configuration_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".lighthouse/rules/long.yaml", OLD_RULE);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 file(s) written, 1 removed"));

    assert!(!dir.path().join(".lighthouse").join("rules").exists());
    assert!(dir.path().join(".lighthouse/decisions/long.yaml").exists());
}

#[test]
fn migrate_keeps_a_rule_file_no_decision_consumed_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let orphan = "select: file\nwhere: 'true'\nmessage: m\n";
    write(dir.path(), ".lighthouse/rules/orphan.yaml", orphan);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .success()
        .stderr(predicate::str::contains("orphan.yaml: kept"));

    let kept = fs::read_to_string(dir.path().join(".lighthouse/rules/orphan.yaml")).unwrap();
    assert_eq!(kept, orphan);
}

#[test]
fn migrate_resolves_a_declared_rule_by_path_not_by_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let rule = "select: file\nwhere: 'true'\nmessage: m\n";
    write(
        dir.path(),
        "cat/core/pack.yaml",
        "id: core\ntitle: Core\nintro: i\nsections: [s]\n",
    );
    write(
        dir.path(),
        "cat/core/s/section.yaml",
        "id: s\ntitle: S\nintro: i\npatterns: [a]\n",
    );
    write(
        dir.path(),
        "cat/core/s/a.yaml",
        "id: core/a\ntitle: A\nintent: i\nscope: file\nrequirement: A MUST b.\nenforcement: mechanical\nimplementation:\n  declarative: core/s/rules/a.yaml\n",
    );
    write(dir.path(), "cat/core/s/rules/a.yaml", rule);
    // Its path ends with the declared one but is another file.
    write(dir.path(), "cat/core/s/extra/core/s/rules/a.yaml", rule);

    lighthouse(dir.path())
        .args(["spec", "migrate", "cat"])
        .assert()
        .success()
        .stderr(predicate::str::contains("extra/core/s/rules/a.yaml: kept"));

    assert!(!dir.path().join("cat/core/s/rules/a.yaml").exists());
    assert!(!dir.path().join("cat/core/s/section.yaml").exists());
    assert!(
        dir.path()
            .join("cat/core/s/extra/core/s/rules/a.yaml")
            .exists()
    );
    let decision = fs::read_to_string(dir.path().join("cat/core/s/a.yaml")).unwrap();
    assert!(decision.contains("type: cel"), "{decision}");
}

#[test]
fn migrate_leaves_other_tools_files_alone() {
    let dir = tempfile::tempdir().unwrap();
    let foreign = [
        (
            ".eslintrc.yml",
            "extends: [eslint:recommended]\nrules: {}\n",
        ),
        ("ci/workflow.yaml", "id: build\nrequirement: none\n"),
        ("docker-compose.yml", "services: {}\n"),
        ("lighthouse.json", "{\"name\": \"not ours\"}\n"),
    ];
    for (name, text) in foreign {
        write(dir.path(), name, text);
    }
    write(dir.path(), "lighthouse.toml", OLD_CONFIG);

    lighthouse(dir.path())
        .args(["spec", "migrate", "."])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 file(s) written, 0 removed"));

    for (name, text) in foreign {
        assert_eq!(fs::read_to_string(dir.path().join(name)).unwrap(), text);
    }
}

#[test]
fn migrate_refuses_keys_it_does_not_know_instead_of_dropping_them() {
    let dir = tempfile::tempdir().unwrap();
    let bogus = format!("{OLD_RULE}bogus: 1\n");
    write(dir.path(), ".lighthouse/rules/long.yaml", &bogus);

    lighthouse(dir.path())
        .args(["spec", "migrate"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown key `bogus`"));
    assert!(dir.path().join(".lighthouse/rules/long.yaml").exists());
}

#[test]
fn a_project_that_still_has_legacy_rules_must_migrate_before_it_runs() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project("plugins = [\"core\"]\n"),
    );
    write(dir.path(), ".lighthouse/rules/long.yaml", OLD_RULE);
    write(dir.path(), "a.txt", "1\n");

    lighthouse(dir.path())
        .arg("check")
        .assert()
        .failure()
        .stderr(predicate::str::contains("lighthouse spec migrate"));
}

#[test]
fn a_dry_run_of_migrate_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lighthouse.toml", OLD_CONFIG);

    lighthouse(dir.path())
        .args(["spec", "migrate", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("would write"));

    assert_eq!(
        fs::read_to_string(dir.path().join("lighthouse.toml")).unwrap(),
        OLD_CONFIG
    );
}

#[test]
fn validate_names_what_is_wrong_across_documents() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\"]\nextends = [\"core/nope\"]\n[rules]\n\"core/nonesuch\" = \"warn\"\n",
        ),
    );
    write(
        dir.path(),
        ".lighthouse/decisions/old.yaml",
        "id: local/old\n",
    );

    lighthouse(dir.path())
        .args(["spec", "validate"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "extends unknown preset `core/nope`",
        ))
        .stdout(predicate::str::contains(
            "rule `core/nonesuch` is not registered",
        ))
        .stdout(predicate::str::contains("lighthouse spec migrate"));
}

#[test]
fn schema_prints_a_kind_in_any_case_and_lists_them() {
    let dir = tempfile::tempdir().unwrap();
    let out = lighthouse(dir.path())
        .args(["schema", "decision"])
        .output()
        .unwrap();
    let schema: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["properties"]["kind"]["const"], "Decision");

    lighthouse(dir.path())
        .arg("schema")
        .assert()
        .success()
        .stdout(predicate::str::contains("Project\tproject.schema.json"));
    lighthouse(dir.path())
        .args(["schema", "nope"])
        .assert()
        .code(2);
}

/// The checked-in schemas are what editors validate documents against, so
/// they must track the Rust types. `UPDATE_SCHEMA=1 cargo test -p
/// lighthouse-cli --test spec` rewrites them.
#[test]
fn checked_in_schemas_match_the_types() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema");
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        lighthouse(root.parent().unwrap())
            .args(["schema", "--write", "schema"])
            .assert()
            .success();
    }
    let schemas = lighthouse_session::schemas();
    assert_eq!(schemas.len(), 9);
    let on_disk = fs::read_dir(&root)
        .unwrap()
        .filter(|e| {
            let name = e.as_ref().unwrap().file_name();
            name.to_string_lossy().ends_with(".schema.json")
        })
        .count();
    assert_eq!(on_disk, 9, "a schema file nothing generates is stale");
    for descriptor in schemas.into_values() {
        let file = root.join(lighthouse_resource::schema_file(descriptor.kind));
        let mut want = serde_json::to_string_pretty(&descriptor.schema).unwrap();
        want.push('\n');
        let have = fs::read_to_string(&file).unwrap_or_default();
        assert!(
            have == want,
            "{} is stale; run `UPDATE_SCHEMA=1 cargo test -p lighthouse-cli --test spec`",
            file.display()
        );
    }
}

/// A fix refuses the fields its `type` does not have, so does the schema.
#[test]
fn the_fix_schema_allows_only_the_fields_of_its_type() {
    let schema = lighthouse_session::schema_of("decision").unwrap().schema;
    let branches = schema["$defs"]["Fix"]["oneOf"].as_array().unwrap();
    assert_eq!(branches.len(), 3);
    for branch in branches {
        assert_eq!(branch["additionalProperties"], false, "{branch}");
        for field in ["safety", "requires"] {
            assert!(branch["properties"][field].is_object(), "{field}: {branch}");
        }
        assert!(
            branch["required"]
                .as_array()
                .unwrap()
                .contains(&"safety".into()),
            "{branch}"
        );
    }
}

#[test]
fn max_warnings_fails_a_run_with_more_warnings_than_allowed() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\"]\n[rules]\n\"core/max-file-lines\" = { level = \"warn\", options = { max = 2 } }\n",
        ),
    );
    write(dir.path(), "a.txt", "1\n2\n3\n");
    lighthouse(dir.path()).arg("check").assert().code(0);
    lighthouse(dir.path())
        .args(["check", "--max-warnings", "1"])
        .assert()
        .code(1);
    lighthouse(dir.path())
        .args(["check", "--max-warnings", "2"])
        .assert()
        .code(0);
    lighthouse(dir.path())
        .args(["check", "--strict"])
        .assert()
        .code(1);
}

#[test]
fn sarif_links_a_rule_to_its_decision_page() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\"]\n[rules]\n\"core/max-file-lines\" = { level = \"warn\", options = { max = 2 } }\n",
        ),
    );
    write(dir.path(), "a.txt", "1\n2\n3\n");
    let out = lighthouse(dir.path())
        .args(["check", "--format", "sarif"])
        .output()
        .unwrap();
    let sarif: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rule = &sarif["runs"][0]["tool"]["driver"]["rules"][0];
    assert!(
        rule["helpUri"]
            .as_str()
            .unwrap()
            .ends_with("docs/decisions/core.md#files-stay-below-a-line-limit")
    );
    assert_eq!(sarif["runs"][0]["results"][0]["level"], "warning");
}
