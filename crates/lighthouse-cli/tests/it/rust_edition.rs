//! `core/formatted` formats a file with the edition of its crate: rustfmt does
//! not read `Cargo.toml`, and `use std::{b, A};` is as rustfmt writes it in the
//! 2021 edition and not in the 2024 one, which orders the names differently.

use std::fs;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{cache_fixtures::write, mcp::Client};

const IMPORT: &str = "use std::{b, A};\n";

/// A workspace of four crates holding the same `use`: `old` of the 2021
/// edition, `inherits` of the one the workspace gives it (2021 too), `new` of
/// the 2024 edition and `none`, which sets none, so it has Cargo's default,
/// 2015, which writes it as 2021 does.
fn workspace() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let plugin = lighthouse_test_support::lang_rust();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"core\", \"rust\"]\n[rules]\n\"core/formatted\" = \"error\"\n",
            plugin.to_str().unwrap()
        )),
    );
    write(
        dir.path(),
        "Cargo.toml",
        "[workspace]\nmembers = [\"old\", \"inherits\", \"new\", \"none\"]\nresolver = \"3\"\n\n[workspace.package]\nedition = \"2021\"\n",
    );
    for (name, edition) in [
        ("old", "edition = \"2021\""),
        ("inherits", "edition.workspace = true"),
        ("new", "edition = \"2024\""),
        ("none", ""),
    ] {
        write(
            dir.path(),
            &format!("{name}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n{edition}\n"),
        );
        write(dir.path(), &format!("{name}/src/lib.rs"), IMPORT);
    }
    dir
}

/// The files the check of `core/formatted` flags.
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
                .map(|f| {
                    f["location"]["path"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned()
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn read(dir: &TempDir, name: &str) -> String {
    fs::read_to_string(dir.path().join(name)).unwrap()
}

#[test]
fn a_file_is_checked_under_the_edition_of_its_crate() {
    let dir = workspace();

    assert_eq!(flagged(&dir), ["new/src/lib.rs"]);
}

#[test]
fn a_file_is_fixed_under_the_edition_of_its_crate() {
    let dir = workspace();

    let fixed: Value = Client::trusting(dir.path())
        .tool("fix", json!({"decision": ["core/formatted"]}))
        .unwrap();

    assert_eq!(
        read(&dir, "new/src/lib.rs"),
        "use std::{A, b};\n",
        "{fixed}"
    );
    assert_eq!(read(&dir, "old/src/lib.rs"), IMPORT, "{fixed}");
    assert_eq!(read(&dir, "none/src/lib.rs"), IMPORT, "{fixed}");
    assert_eq!(read(&dir, "inherits/src/lib.rs"), IMPORT, "{fixed}");
    assert!(flagged(&dir).is_empty());
}
