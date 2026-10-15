//! Every fact and function a check can read declares how far it reaches, so
//! that a new one cannot forget to, and the rules of the bundled packs get
//! the reach their expressions earn.

use std::{collections::BTreeSet, fs, path::Path};

use lighthouse_checks::reach::{FACTS, FUNCTIONS};
use lighthouse_model::Reach;

fn source(relative: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join(relative),
    )
    .unwrap()
}

/// The string literals that follow `marker` (up to the closing quote) in the
/// files under `src` named by `relative`.
fn literals(text: &str, marker: &str) -> BTreeSet<String> {
    text.match_indices(marker)
        .filter_map(|(at, _)| {
            let rest = &text[at + marker.len()..];
            Some(rest[..rest.find('"')?].to_owned())
        })
        .collect()
}

/// The source of every file of the builder and of the facts it reads from.
fn builder_text() -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/builder");
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    let mut text: String = files
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect();
    text.push_str(&source("facts.rs"));
    text
}

#[test]
fn every_fact_the_builder_derives_declares_its_reach() {
    let declared: BTreeSet<&str> = FACTS.iter().map(|(word, _)| *word).collect();
    let mentioned = literals(&builder_text(), ".fact(\"");
    assert!(mentioned.len() > 15, "{mentioned:?}");
    let undeclared: Vec<_> = mentioned
        .iter()
        .filter(|word| !declared.contains(word.as_str()))
        .collect();
    assert!(
        undeclared.is_empty(),
        "facts without a reach: {undeclared:?}"
    );
}

#[test]
fn every_function_of_the_library_declares_its_reach() {
    let declared: BTreeSet<&str> = FUNCTIONS.iter().map(|(name, _)| *name).collect();
    let library = source("library/mod.rs") + &source("library/limits.rs");
    let mut defined = literals(&library, "add_function(\"");
    let table = library.split("FUNCTIONS: [(&str, &str); ").nth(1).unwrap();
    defined.extend(literals(table.split("];").next().unwrap(), "    (\""));
    assert!(defined.len() > 20, "{defined:?}");
    let undeclared: Vec<_> = defined
        .iter()
        .filter(|name| !declared.contains(name.as_str()))
        .collect();
    assert!(
        undeclared.is_empty(),
        "functions without a reach: {undeclared:?}"
    );
}

/// Every bundled rule: its reach and whether it reads the place of
/// neighbors, or `None` when the host runs it every time.
const REACHES: &[(&str, Option<(Reach, bool)>)] = &[
    ("core/allow", None),
    ("core/decision-naming", Some((Reach::Global, false))),
    ("core/file-size", Some((Reach::Local, false))),
    ("core/no-unused-allow", None),
    // A rule with a review route is run every time: the models keep their
    // own answers.
    ("design/comments", None),
    ("design/complexity", None),
    ("design/concurrency", Some((Reach::Local, false))),
    ("design/constructors", None),
    ("design/declaration-groups", Some((Reach::Local, false))),
    ("design/errors", Some((Reach::Local, false))),
    ("design/exported-doc", Some((Reach::Neighbors, false))),
    ("design/layers", Some((Reach::Global, false))),
    ("design/layout", None),
    ("design/naming", None),
    ("design/ownership", None),
    ("design/prefer-method", None),
    ("design/signature", None),
    ("design/symbols", None),
    ("testing/cases", None),
    ("testing/coverage", None),
    ("testing/file-layout", Some((Reach::Neighbors, true))),
    ("testing/owner", Some((Reach::Global, false))),
    ("testing/public-boundary", Some((Reach::Global, false))),
    ("testing/reliability", None),
];

#[test]
fn rules_reach_as_far_as_what_they_read() {
    let registry = lighthouse_checks::registry();
    let found: Vec<(String, Option<(Reach, bool)>)> = registry
        .rules()
        .map(|rule| {
            let meta = rule.manifest();
            let caching = meta.caching.as_ref();
            (
                meta.id.clone(),
                caching.map(|caching| (caching.reach, caching.positions)),
            )
        })
        .collect();
    let expected: Vec<_> = REACHES
        .iter()
        .map(|(id, reach)| ((*id).to_owned(), *reach))
        .collect();
    assert_eq!(found, expected);
}
