//! The fragment cache: a warm run answers exactly like a cold one, and an edit
//! analyzes again only what it can change.

use std::{collections::BTreeMap, fs, path::Path};

use serde_json::{Value, json};

use crate::index::{index_with, project};

const MANIFEST: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n";
const LIB: &str = "pub mod a;\npub mod b;\n";
const A: &str = "pub fn one() -> u32 { 1 }\n";
const B: &str = "pub fn two() -> u32 { crate::a::one() + 1 }\n";

fn demo() -> tempfile::TempDir {
    project(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", LIB),
        ("src/a.rs", A),
        ("src/b.rs", B),
    ])
}

fn json_of(dir: &Path, cache: Option<&Path>) -> Value {
    serde_json::to_value(index_with(dir, json!({}), cache)).unwrap()
}

/// The key of every file in the cache, which changes exactly when the file is
/// analyzed again.
fn keys(cache: &Path) -> BTreeMap<String, String> {
    let stored: Value =
        serde_json::from_slice(&fs::read(cache.join("fragments.json")).unwrap()).unwrap();
    stored["files"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(file, entry)| (file.clone(), entry["key"].as_str().unwrap().to_owned()))
        .collect()
}

fn changed(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<String> {
    after
        .iter()
        .filter(|(file, key)| before.get(*file) != Some(*key))
        .map(|(file, _)| file.clone())
        .collect()
}

#[test]
fn a_warm_run_answers_exactly_like_a_cold_one() {
    let dir = demo();
    let cache = tempfile::tempdir().unwrap();
    let plain = json_of(dir.path(), None);

    let cold = json_of(dir.path(), Some(cache.path()));
    let warm = json_of(dir.path(), Some(cache.path()));

    assert_eq!(cold, plain);
    assert_eq!(warm, plain);
}

#[test]
fn a_body_edit_analyzes_only_that_file() {
    let dir = demo();
    let cache = tempfile::tempdir().unwrap();
    json_of(dir.path(), Some(cache.path()));
    let before = keys(cache.path());

    fs::write(dir.path().join("src/a.rs"), "pub fn one() -> u32 { 10 }\n").unwrap();
    let warm = json_of(dir.path(), Some(cache.path()));

    assert_eq!(warm, json_of(dir.path(), None));
    assert_eq!(changed(&before, &keys(cache.path())), ["src/a.rs"]);
}

#[test]
fn a_signature_edit_analyzes_every_file_again() {
    let dir = demo();
    let cache = tempfile::tempdir().unwrap();
    json_of(dir.path(), Some(cache.path()));
    let before = keys(cache.path());

    fs::write(dir.path().join("src/a.rs"), "pub fn one() -> u64 { 1 }\n").unwrap();
    let warm = json_of(dir.path(), Some(cache.path()));

    assert_eq!(warm, json_of(dir.path(), None));
    assert_eq!(changed(&before, &keys(cache.path())).len(), 3);
}

#[test]
fn an_added_or_removed_file_analyzes_every_file_again() {
    let dir = demo();
    let cache = tempfile::tempdir().unwrap();
    json_of(dir.path(), Some(cache.path()));
    let before = keys(cache.path());

    fs::write(
        dir.path().join("src/lib.rs"),
        "pub mod a;\npub mod b;\npub mod c;\n",
    )
    .unwrap();
    fs::write(dir.path().join("src/c.rs"), "pub fn three() {}\n").unwrap();
    let added = json_of(dir.path(), Some(cache.path()));
    assert_eq!(added, json_of(dir.path(), None));
    assert_eq!(changed(&before, &keys(cache.path())).len(), 4);

    fs::write(dir.path().join("src/lib.rs"), LIB).unwrap();
    fs::remove_file(dir.path().join("src/c.rs")).unwrap();
    assert_eq!(
        json_of(dir.path(), Some(cache.path())),
        json_of(dir.path(), None)
    );
}

#[test]
fn a_manifest_edit_analyzes_every_file_again() {
    let dir = demo();
    let cache = tempfile::tempdir().unwrap();
    json_of(dir.path(), Some(cache.path()));
    let before = keys(cache.path());

    fs::write(
        dir.path().join("Cargo.toml"),
        format!("{MANIFEST}publish = false\n"),
    )
    .unwrap();
    let warm = json_of(dir.path(), Some(cache.path()));

    assert_eq!(warm, json_of(dir.path(), None));
    assert_eq!(changed(&before, &keys(cache.path())).len(), 3);
}

#[test]
fn an_unusable_cache_directory_still_gives_the_same_result() {
    let dir = demo();
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let unusable = blocker.path().join("cache");

    let mut got = json_of(dir.path(), Some(&unusable));

    let mut plain = json_of(dir.path(), None);
    let notice = got["notices"].as_array_mut().unwrap().pop().unwrap();
    assert!(
        notice.as_str().unwrap().contains("not writable"),
        "{notice}"
    );
    plain["notices"] = got["notices"].clone();
    assert_eq!(got, plain);
}
