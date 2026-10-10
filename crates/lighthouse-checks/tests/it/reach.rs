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

fn builder_text() -> String {
    [
        "mod",
        "api",
        "effective",
        "forward",
        "hidden",
        "homonyms",
        "ownership",
        "role",
    ]
    .iter()
    .map(|name| source(&format!("builder/{name}.rs")))
    .collect()
}

#[test]
fn every_fact_the_builder_derives_declares_its_reach() {
    let declared: BTreeSet<&str> = FACTS.iter().map(|(word, _)| *word).collect();
    let text = builder_text();
    let mentioned = literals(&text, "mentions(\"");
    let undeclared: Vec<_> = mentioned
        .iter()
        .filter(|word| !declared.contains(word.as_str()))
        .collect();
    assert!(
        undeclared.is_empty(),
        "facts without a reach: {undeclared:?}"
    );
    // The words of the `role` fact are mentioned in a list.
    for word in ["role", "limit(", "counted(", "built"] {
        assert!(text.contains(&format!("\"{word}\"")), "{word}");
    }
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

#[test]
fn rules_reach_as_far_as_what_they_read() {
    let registry = lighthouse_checks::registry();
    let reach = |id: &str| {
        registry
            .rule(id)
            .unwrap_or_else(|| panic!("no rule {id}"))
            .caching()
            .map(|caching| caching.reach)
    };
    // Reads the project: an index over every public type.
    assert_eq!(reach("design/unique-type-names"), Some(Reach::Global));
    // A module is a project subject.
    assert_eq!(reach("design/tiny-modules"), Some(Reach::Global));
    // Reads only the file and its own symbols.
    assert_eq!(reach("core/max-lines"), Some(Reach::Local));
    // A program that reads more than the code model is run every time.
    let uncached = registry
        .rules()
        .filter(|rule| rule.caching().is_none())
        .count();
    let total = registry.rules().count();
    assert!(
        uncached < total,
        "{uncached} of {total} rules are not cached"
    );
}
