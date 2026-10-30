//! Every fact and function a check can read declares how far it reaches, so
//! that a new one cannot forget to, and the rules of the bundled packs get
//! the reach their expressions earn.

use std::{collections::BTreeSet, fs, path::Path};

use lighthouse_checks::caching::FACTS;
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
    let mentioned = literals(&builder_text(), ".field(\"");
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

/// Every bundled rule: its reach and whether it reads the place of
/// neighbors, or `None` when the host runs it every time.
const REACHES: &[(&str, Option<(Reach, bool)>)] = &[
    ("core/allow-reason", None),
    ("core/decision-naming", Some((Reach::Global, false))),
    ("core/formatted", None),
    ("core/max-len", Some((Reach::Local, false))),
    ("core/max-lines", Some((Reach::Local, false))),
    ("core/no-unused-allow", None),
    ("design/abbreviations", Some((Reach::Local, false))),
    ("design/abstraction-level", Some((Reach::Local, false))),
    ("design/complexity", Some((Reach::Global, false))),
    ("design/consistent-terms", Some((Reach::Global, false))),
    ("design/constructor-validation", Some((Reach::Local, false))),
    ("design/consumer-interfaces", Some((Reach::Local, false))),
    ("design/context-first", Some((Reach::Local, false))),
    ("design/coupling", Some((Reach::Neighbors, false))),
    ("design/declaration-groups", Some((Reach::Local, false))),
    ("design/encapsulated-platform", Some((Reach::Local, false))),
    ("design/encapsulated-state", Some((Reach::Local, false))),
    ("design/error-categories", Some((Reach::Local, false))),
    ("design/error-identity", Some((Reach::Local, false))),
    ("design/error-translation", Some((Reach::Local, false))),
    ("design/exported-doc", Some((Reach::Neighbors, false))),
    ("design/feature-envy", Some((Reach::Neighbors, false))),
    ("design/general-extraction", Some((Reach::Local, false))),
    ("design/layers", Some((Reach::Global, false))),
    ("design/max-name-words", Some((Reach::Local, false))),
    ("design/meaningful-separation", Some((Reach::Global, false))),
    ("design/module-layout", Some((Reach::Global, false))),
    ("design/narrow-inputs", Some((Reach::Local, false))),
    ("design/no-caller-dependency", Some((Reach::Local, false))),
    ("design/no-conjoined-modules", Some((Reach::Global, false))),
    ("design/no-detached-comment", Some((Reach::Local, false))),
    ("design/no-duplication", Some((Reach::Local, false))),
    ("design/no-generated-edits", Some((Reach::Local, false))),
    ("design/no-hybrid-type", Some((Reach::Local, false))),
    ("design/no-mutable-globals", Some((Reach::Neighbors, false))),
    ("design/no-noise-comment", Some((Reach::Local, false))),
    ("design/no-panic", Some((Reach::Local, false))),
    ("design/no-private-types", Some((Reach::Neighbors, false))),
    (
        "design/no-redundant-qualifier",
        Some((Reach::Neighbors, false)),
    ),
    ("design/no-redundant-symbol", Some((Reach::Global, false))),
    (
        "design/no-speculative-generality",
        Some((Reach::Local, false)),
    ),
    ("design/no-state-in-errors", Some((Reach::Local, false))),
    ("design/no-stored-context", Some((Reach::Local, false))),
    ("design/no-todo", Some((Reach::Local, false))),
    ("design/no-type-encoding", Some((Reach::Local, false))),
    ("design/no-unused-symbol", Some((Reach::Neighbors, false))),
    ("design/optional-options", Some((Reach::Local, false))),
    ("design/owner-file", Some((Reach::Neighbors, false))),
    ("design/plural-collections", Some((Reach::Local, false))),
    ("design/predicate-names", Some((Reach::Local, false))),
    ("design/prefer-method", Some((Reach::Neighbors, false))),
    ("design/prefer-reuse", Some((Reach::Local, false))),
    ("design/reserved-verbs", Some((Reach::Local, false))),
    ("design/resource-owner", Some((Reach::Local, false))),
    ("design/scoped-borrows", Some((Reach::Local, false))),
    ("design/short-names", Some((Reach::Local, false))),
    ("design/signature-size", Some((Reach::Global, false))),
    ("design/single-responsibility", Some((Reach::Global, false))),
    ("design/state-owner", Some((Reach::Local, false))),
    ("design/stateless-layout", Some((Reach::Local, false))),
    ("design/task-shutdown", Some((Reach::Local, false))),
    ("design/top-down-order", Some((Reach::Global, false))),
    ("design/vertical-distance", Some((Reach::Local, false))),
    ("markdown/gfm", None),
    ("markdown/links", None),
    ("testing/behavior-cases", Some((Reach::Local, false))),
    ("testing/case-assertions", Some((Reach::Local, false))),
    ("testing/consistent-case-style", Some((Reach::Local, false))),
    ("testing/contract-coverage", Some((Reach::Local, false))),
    ("testing/file-layout", Some((Reach::Neighbors, true))),
    ("testing/independent-golden", Some((Reach::Local, false))),
    ("testing/independent-tests", Some((Reach::Local, false))),
    ("testing/lowest-layer", Some((Reach::Local, false))),
    ("testing/max-case-depth", Some((Reach::Local, false))),
    ("testing/no-duplicate-owner", Some((Reach::Global, false))),
    ("testing/no-hidden-target", Some((Reach::Neighbors, false))),
    ("testing/no-missing-owner", Some((Reach::Global, false))),
    (
        "testing/no-polling-after-teardown",
        Some((Reach::Local, false)),
    ),
    ("testing/no-structure-tests", Some((Reach::Local, false))),
    ("testing/path-parity", Some((Reach::Local, false))),
    ("testing/prefer-table", Some((Reach::Local, false))),
    ("testing/public-boundary", Some((Reach::Global, false))),
    ("testing/pure-polling", Some((Reach::Local, false))),
    ("testing/scenario-cases", Some((Reach::Local, false))),
    ("testing/self-validating", Some((Reach::Local, false))),
    ("testing/simple-tables", Some((Reach::Local, false))),
    ("testing/specific-evidence", Some((Reach::Local, false))),
    ("testing/standard-assertions", Some((Reach::Global, false))),
];

#[test]
fn rules_reach_as_far_as_what_they_read() {
    let registry = lighthouse_test_support::full_registry();
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
