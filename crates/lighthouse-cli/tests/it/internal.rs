//! Symbols exported only inside the project: the Go test decisions judge them
//! (an `internal/` package is API for the rest of the module), Rust does not (there `Internal`
//! includes `pub(crate)`).

use std::{fs, path::Path};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const DOC: &str = "design/exported-doc";
const OWNER: &str = "testing/owner-test";
const SINGLE: &str = "testing/single-owner-test";

const PACKS: &str = "\"core\", \"design\", \"testing\"";
const PRESETS: &str = "[\"core/recommended\", \"design/recommended\", \"testing/recommended\"]";

fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// `(rule, message)` of every finding the project reports.
fn findings(dir: &Path) -> Vec<(String, String)> {
    let out = Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(dir)
        .args(["check", "--no-store", "--format", "agent-json"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut found = Vec::new();
    for group in report["groups"].as_array().unwrap() {
        for rows in group["files"].as_object().unwrap().values() {
            for row in rows.as_array().unwrap() {
                found.push((
                    group["rule"].as_str().unwrap().to_owned(),
                    row[1].as_str().unwrap().to_owned(),
                ));
            }
        }
    }
    found
}

fn messages<'a>(found: &'a [(String, String)], rule: &str) -> Vec<&'a str> {
    found
        .iter()
        .filter(|(r, _)| r == rule)
        .map(|(_, message)| message.as_str())
        .collect()
}

fn go_project() -> Option<TempDir> {
    let plugin = lighthouse_test_support::lang_go()?;
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [{PACKS}, {{ id = \"lang-go\", path = {:?} }}]\nextends = {PRESETS}\n",
            plugin.to_str().unwrap()
        )),
    );
    write(dir.path(), "go.mod", "module example.com/app\n\ngo 1.26\n");
    Some(dir)
}

#[test]
fn go_judges_the_exported_symbols_of_an_internal_package() {
    let Some(dir) = go_project() else {
        return;
    };
    write(
        dir.path(),
        "internal/foo/foo.go",
        "package foo\n\nfunc Open() {}\n\ntype Store struct{}\n\n// Get returns one.\nfunc (s *Store) Get() int { return 1 }\n",
    );
    write(
        dir.path(),
        "internal/foo/foo_test.go",
        "package foo_test\n\nimport (\n\t\"testing\"\n\n\t\"example.com/app/internal/foo\"\n)\n\nfunc TestOpen(t *testing.T) { foo.Open() }\n\nfunc TestOpen_Missing(t *testing.T) { foo.Open() }\n",
    );
    let found = findings(dir.path());

    assert!(
        messages(&found, DOC).is_empty(),
        "exported-doc keeps skipping internal symbols: {found:?}"
    );
    let untested = messages(&found, OWNER);
    assert!(
        untested.iter().any(|m| m.contains("type Store")),
        "{found:?}"
    );
    let doubled = messages(&found, SINGLE);
    assert!(
        doubled.iter().any(|m| m.contains("function Open")),
        "{found:?}"
    );
}

#[test]
fn rust_does_not_judge_crate_visible_items() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = lighthouse_test_support::lang_rust();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [{PACKS}, {{ id = \"lang-rust\", path = {:?} }}]\nextends = {PRESETS}\n",
            plugin.to_str().unwrap()
        )),
    );
    write(
        dir.path(),
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        dir.path(),
        "src/lib.rs",
        "/// Opens.\npub fn open() {}\n\npub(crate) fn crate_wide() {}\n\npub(crate) struct Hidden;\n\nimpl Hidden {\n    pub(crate) fn run(&self) {}\n}\n",
    );
    write(
        dir.path(),
        "tests/open.rs",
        "#[test]\nfn open() {\n    demo::open();\n}\n",
    );
    let found = findings(dir.path());
    for rule in [DOC, OWNER, SINGLE] {
        let hit = messages(&found, rule);
        assert!(
            hit.iter()
                .all(|m| !m.contains("crate_wide") && !m.contains("Hidden") && !m.contains("run")),
            "{rule}: {hit:?}"
        );
    }
}
