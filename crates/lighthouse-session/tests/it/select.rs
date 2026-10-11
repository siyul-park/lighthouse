//! What `--decision` selects, and the shortest fingerprint prefix.

use lighthouse_session::{Selector, long_enough};

const KNOWN: [&str; 4] = [
    "design/coupling",
    "design/exported-doc",
    "core/max-lines",
    "testing/unique-owner",
];

fn selector(patterns: &[&str]) -> Selector {
    let patterns: Vec<String> = patterns.iter().map(|p| (*p).to_owned()).collect();
    Selector::new(&patterns).unwrap()
}

#[test]
fn no_pattern_selects_everything() {
    let all = selector(&[]);
    assert!(all.is_empty());
    assert!(KNOWN.iter().all(|id| all.matches(id)));
}

#[test]
fn an_id_selects_itself_and_a_glob_its_pack() {
    let one = selector(&["core/max-lines"]);
    assert!(one.matches("core/max-lines") && !one.matches("core/other"));
    let pack = selector(&["design/*"]);
    assert!(pack.matches("design/coupling") && !pack.matches("core/max-lines"));
    assert!(!pack.matches("design/a/b"), "a glob does not cross `/`");
}

#[test]
fn expansion_sorts_and_removes_duplicates() {
    let ids = selector(&["design/*", "design/coupling", "core/max-lines"])
        .expand(KNOWN)
        .unwrap();
    assert_eq!(
        ids,
        ["core/max-lines", "design/coupling", "design/exported-doc"]
    );
}

#[test]
fn a_glob_that_matches_nothing_is_an_error() {
    let error = selector(&["nope/*"]).expand(KNOWN).unwrap_err();
    assert!(error.to_string().contains("no decision matches `nope/*`"));
}

#[test]
fn a_fingerprint_prefix_needs_seven_characters() {
    assert!(long_enough("abcdef0").is_ok());
    assert!(long_enough("abcdef0123").is_ok());
    let error = long_enough("abcdef").unwrap_err().to_string();
    assert!(error.contains("give at least 7 characters"), "{error}");
    assert!(long_enough("").is_err());
}
