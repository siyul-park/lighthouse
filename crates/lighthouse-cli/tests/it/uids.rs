//! Uids, source directives and decision names through the binary:
//! `spec migrate`, `spec validate` and `check` over small projects.

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

const UNIDENTIFIED: &str = "# yaml-language-server: $schema=../../schema/decision.schema.json
apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/plain # keep this comment
  labels: {}
spec:
  title: Plain
  context: A plain decision.
  scope: { subject: file }
  requirement: A file MAY be plain.
";

fn uid_of(text: &str) -> String {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("uid: "))
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn migrate_gives_every_decision_a_uid_once_and_keeps_the_rest_of_the_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".lighthouse/decisions/plain.yaml", UNIDENTIFIED);

    lighthouse(dir.path())
        .args(["spec", "migrate", ".lighthouse"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 file(s) written"));

    let path = dir.path().join(".lighthouse/decisions/plain.yaml");
    let migrated = fs::read_to_string(&path).unwrap();
    let uid = uid_of(&migrated);
    assert!(lighthouse_resource::is_uid(&uid), "{migrated}");
    assert_eq!(
        migrated.replace(&format!("  uid: {uid}\n"), ""),
        UNIDENTIFIED,
        "only the uid line is new"
    );

    lighthouse(dir.path())
        .args(["spec", "migrate", ".lighthouse"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 file(s) written"));
    assert_eq!(uid_of(&fs::read_to_string(&path).unwrap()), uid);
}

#[test]
fn migrate_rewrites_allow_comments_in_source_files_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/lib.rs",
        r####"// lighthouse:allow design/exported-doc -- shim
pub fn open() {} // lighthouse-disable-line design/section-banners -- trailing

/* lighthouse:allow a, b -- two */
fn quoted() -> &'static str {
    "// lighthouse:allow x -- in a string"
}
fn raw() -> &'static str {
    r#"
// lighthouse:allow y -- in a raw string
    "#
}
fn lifetime<'a>(s: &'a str) -> &'a str { s } // lighthouse-enable design/exported-doc
// Write lighthouse:allow x -- in prose
"####,
    );
    write(
        dir.path(),
        "main.go",
        "package main\n\n\t// lighthouse:allow design/exported-doc\nfunc Open() {}\nvar raw = `\n// lighthouse:allow z -- in a raw string\n`\n",
    );
    write(dir.path(), "NOTES.md", "lighthouse:allow design/a -- doc\n");

    lighthouse(dir.path())
        .args(["spec", "migrate", "."])
        .assert()
        .success();

    let rust = fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        rust.starts_with("// lighthouse-disable-next-line design/exported-doc -- shim\n"),
        "{rust}"
    );
    assert!(
        rust.contains("pub fn open() {} // lighthouse-disable-line design/no-banners -- trailing"),
        "a trailing directive is migrated too: {rust}"
    );
    assert!(
        rust.contains("/* lighthouse-disable-next-line a, b -- two */"),
        "{rust}"
    );
    assert!(
        rust.contains("\"// lighthouse:allow x -- in a string\""),
        "{rust}"
    );
    assert!(
        rust.contains("\n// lighthouse:allow y -- in a raw string\n"),
        "{rust}"
    );
    assert!(
        rust.contains("// Write lighthouse:allow x -- in prose"),
        "{rust}"
    );
    let go = fs::read_to_string(dir.path().join("main.go")).unwrap();
    assert!(
        go.contains("\t// lighthouse-disable-next-line design/exported-doc\n"),
        "{go}"
    );
    assert!(
        go.contains("\n// lighthouse:allow z -- in a raw string\n"),
        "{go}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("NOTES.md")).unwrap(),
        "lighthouse:allow design/a -- doc\n"
    );

    lighthouse(dir.path())
        .args(["spec", "migrate", "."])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 file(s) written"));
}

#[test]
fn migrate_renames_the_ids_of_trailing_and_block_directives() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/lib.rs",
        "pub fn open() {} // lighthouse-disable-line core/max-file-lines, design/exported-doc -- why\n/* lighthouse-enable core/max-file-lines */\nfn s() -> &'static str { \"// lighthouse-disable-line core/max-file-lines -- text\" }\n",
    );

    lighthouse(dir.path())
        .args(["spec", "migrate", "."])
        .assert()
        .success();

    let rust = fs::read_to_string(dir.path().join("src/lib.rs")).unwrap();
    assert!(
        rust.contains("// lighthouse-disable-line core/max-lines, design/exported-doc -- why"),
        "{rust}"
    );
    assert!(
        rust.contains("/* lighthouse-enable core/max-lines */"),
        "{rust}"
    );
    assert!(
        rust.contains("\"// lighthouse-disable-line core/max-file-lines -- text\""),
        "{rust}"
    );
}

#[test]
fn validate_asks_a_decision_without_a_uid_to_be_migrated() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), ".lighthouse/decisions/plain.yaml", UNIDENTIFIED);

    lighthouse(dir.path())
        .args(["spec", "validate", ".lighthouse/decisions"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains(
            "local/plain: has no `metadata.uid`",
        ));

    lighthouse(dir.path())
        .args(["spec", "migrate", ".lighthouse"])
        .assert()
        .success();
    lighthouse(dir.path())
        .args(["spec", "validate", ".lighthouse/decisions"])
        .assert()
        .success();
}

#[test]
fn decision_naming_reports_each_decision_of_a_file_at_its_own_line() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "plugins = [\"core\"]\nextends = [\"core/recommended\"]\n",
        ),
    );
    let doc = |name: &str| {
        format!(
            "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  labels: {{}}\n  name: {name}\n  uid: 3f1c4a52-9b0e-4e6a-8f55-6a1d0c2b7e90\nspec:\n  title: A\n  name: not-this\n"
        )
    };
    write(
        dir.path(),
        "docs/many.yaml",
        &format!(
            "{}---\n{}",
            doc("p/order-is-not-a-reason"),
            doc("p/when-not-be")
        ),
    );
    write(
        dir.path(),
        "testdata/skipped.yaml",
        &doc("p/order-is-not-a-reason"),
    );
    write(
        dir.path(),
        "docs/broken.yaml",
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata: [\n",
    );

    let out = lighthouse(dir.path())
        .args(["check", "--no-store"])
        .output()
        .unwrap();

    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains(
            "docs/many.yaml:5:9: warn core/decision-naming: decision name `p/order-is-not-a-reason`"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "docs/many.yaml:15:9: warn core/decision-naming: decision name `p/when-not-be`"
        ),
        "{text}"
    );
    assert!(!text.contains("skipped.yaml"), "{text}");
    let notices = String::from_utf8(out.stderr).unwrap();
    assert!(
        notices.contains("docs/broken.yaml: not read as a document"),
        "{notices}"
    );
}
