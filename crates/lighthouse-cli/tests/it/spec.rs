//! `lighthouse spec validate` and `schema` through the binary,
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
            "extends unknown project `core/nope`",
        ))
        .stdout(predicate::str::contains(
            "rule `core/nonesuch` is not registered",
        ))
        .stdout(predicate::str::contains("no `kind`"));
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
    assert_eq!(schemas.len(), 7);
    let on_disk = fs::read_dir(&root)
        .unwrap()
        .filter(|e| {
            let name = e.as_ref().unwrap().file_name();
            name.to_string_lossy().ends_with(".schema.json")
        })
        .count();
    assert_eq!(on_disk, 7, "a schema file nothing generates is stale");
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
            "plugins = [\"core\"]\n[rules]\n\"core/max-lines\" = { level = \"warn\", options = { max = 2 } }\n",
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
            "plugins = [\"core\"]\n[rules]\n\"core/max-lines\" = { level = \"warn\", options = { max = 2 } }\n",
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
