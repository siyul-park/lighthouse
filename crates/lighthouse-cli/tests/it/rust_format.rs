//! `core/formatted` on Rust judges each file by its own text (stdin, in the
//! directory of the file), as the batch of a first run judges it: a module a
//! parent skips, and a `rustfmt.toml` below the root, give the same findings
//! cold and warm.

use std::fs;

use serde_json::json;
use tempfile::TempDir;

use crate::{cache_fixtures::write, mcp::Client};

/// A crate of two files and a nested one with a `rustfmt.toml` of its own.
fn project() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let plugin = lighthouse_test_support::lang_rust();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}]\n[rules]\n\"core/formatted\" = \"error\"\n",
            plugin.to_str().unwrap()
        )),
    );
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"top\", \"nested\"]\nresolver = \"3\"\n",
    );
    for name in ["top", "nested"] {
        write(
            dir.path(),
            &format!("{name}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
    }
    write(
        dir.path(),
        "top/src/lib.rs",
        "#[rustfmt::skip]\nmod skipped;\nmod plain;\n",
    );
    write(dir.path(), "top/src/skipped.rs", "pub fn  a( ) {}\n");
    write(dir.path(), "top/src/plain.rs", "pub fn b() {}\n");
    write(dir.path(), "nested/rustfmt.toml", "max_width = 30\n");
    write(
        dir.path(),
        "nested/src/lib.rs",
        "pub fn call() -> u32 { helper(1, 2, 3) }\nfn helper(a: u32, b: u32, c: u32) -> u32 { a + b + c }\n",
    );
    dir
}

fn flagged(dir: &TempDir) -> Vec<String> {
    let found = Client::trusting(dir.path())
        .tool(
            "check",
            json!({"decision": ["core/formatted"], "detail": "full"}),
        )
        .unwrap();
    let mut files: Vec<String> = found["findings"]
        .as_array()
        .map(|all| {
            all.iter()
                .filter_map(|f| f["location"]["path"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

/// Changes the text of a file without formatting it, so that its next check
/// is a run of the file alone.
fn touch(dir: &TempDir, name: &str) {
    let path = dir.path().join(name);
    let text = fs::read_to_string(&path).unwrap();
    fs::write(path, format!("{text}// touched\n")).unwrap();
}

#[test]
fn a_module_the_parent_skips_is_judged_by_its_own_text_cold_and_warm() {
    let dir = project();
    let cold = flagged(&dir);
    assert!(cold.contains(&"top/src/skipped.rs".to_owned()), "{cold:?}");
    assert!(!cold.contains(&"top/src/plain.rs".to_owned()), "{cold:?}");

    touch(&dir, "top/src/skipped.rs");
    touch(&dir, "nested/src/lib.rs");
    touch(&dir, "top/src/plain.rs");
    assert_eq!(flagged(&dir), cold);
}

#[test]
fn a_rustfmt_toml_below_the_root_decides_for_the_files_under_it_cold_and_warm() {
    let dir = project();
    let cold = flagged(&dir);
    assert!(cold.contains(&"nested/src/lib.rs".to_owned()), "{cold:?}");

    touch(&dir, "nested/src/lib.rs");
    touch(&dir, "top/src/plain.rs");
    assert_eq!(flagged(&dir), cold);

    let fixed = Client::trusting(dir.path())
        .tool("fix", json!({"decision": ["core/formatted"]}))
        .unwrap();
    assert!(flagged(&dir).is_empty(), "{fixed}");
}

#[test]
fn a_rustfmt_toml_in_a_module_directory_of_the_crate_gives_the_same_findings_cold_and_warm() {
    let dir = project();
    write(dir.path(), "top/src/lib.rs", "mod sub;\n");
    write(dir.path(), "top/src/sub/rustfmt.toml", "max_width = 30\n");
    // Formatted under the width 30 of its directory, not under the default.
    write(
        dir.path(),
        "top/src/sub/mod.rs",
        "pub fn f() {\n    g(\n        1111111111,\n        2222222222,\n        3333333333,\n    );\n}\n",
    );
    let cold = flagged(&dir);
    assert!(!cold.contains(&"top/src/sub/mod.rs".to_owned()), "{cold:?}");

    touch(&dir, "top/src/sub/mod.rs");
    touch(&dir, "top/src/lib.rs");
    assert_eq!(flagged(&dir), cold);
}
