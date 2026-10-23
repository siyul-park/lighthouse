//! The `kinds` option of a decision that reports several kinds: the shape
//! `spec validate` holds it to, and the `strict` project derived from it.

use std::{path::Path, sync::Arc};

use lighthouse_model::Severity;
use lighthouse_spec::{Catalog, Config, Rules};
use lighthouse_test_support::catalog::*;
use serde_json::json;

const CHECK: &str = "  check:
    type: cel
    select: file
    where: 'true'
    message: m
  examples:
    - name: bad
      language: go
      kind: invalid
      files: [{ path: a.go, body: x }]
    - name: good
      language: go
      kind: valid
      files: [{ path: a.go, body: y }]
";

/// A decision with a `kinds` option written as `kinds` says; `extra` follows
/// its `check`.
fn with_kinds(kinds: &str) -> String {
    format!(
        "  options:\n    properties:\n      kinds:\n{kinds}        description: d\n    additionalProperties: false\n{CHECK}"
    )
}

const NARROW: &str =
    "        type: array\n        items: { enum: [a, b, c] }\n        default: [a]\n";

fn rejected(kinds: &str, needle: &str) {
    let err = Catalog::from_files(
        decision_with(&with_kinds(kinds)),
        &lighthouse_test_support::cel(),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains(needle),
        "{err} should mention {needle}"
    );
}

#[test]
fn a_kinds_option_lists_unique_strings_and_defaults_to_some_of_them() {
    Catalog::from_files(
        decision_with(&with_kinds(NARROW)),
        &lighthouse_test_support::cel(),
    )
    .unwrap();
    let all = "        type: array\n        items: { enum: [a, b] }\n        default: [a, b]\n";
    Catalog::from_files(
        decision_with(&with_kinds(all)),
        &lighthouse_test_support::cel(),
    )
    .unwrap();
}

#[test]
fn a_kinds_option_must_be_a_list_with_items() {
    rejected(
        "        type: string\n        default: a\n",
        "must be a list",
    );
    rejected(
        "        type: array\n        default: []\n",
        "must be a list",
    );
}

#[test]
fn the_items_of_a_kinds_option_enumerate_unique_non_empty_strings() {
    for items in [
        "{ type: string }",
        "{ enum: [] }",
        "{ enum: [a, a] }",
        "{ enum: [a, 1] }",
        "{ enum: [a, ''] }",
    ] {
        rejected(
            &format!("        type: array\n        items: {items}\n        default: []\n"),
            "unique, non-empty strings",
        );
    }
}

#[test]
fn the_default_of_a_kinds_option_is_a_subset_of_its_items() {
    rejected(
        "        type: array\n        items: { enum: [a, b] }\n        default: [a, z]\n",
        "default that is not",
    );
}

#[test]
fn a_value_of_a_kinds_option_is_one_of_the_declared_kinds() {
    let catalog = Catalog::from_files(
        decision_with(&with_kinds(NARROW)),
        &lighthouse_test_support::cel(),
    )
    .unwrap();
    let decision = catalog.decision("p/a").unwrap();
    let kinds = |value| lighthouse_model::Options::from_iter([("kinds".to_owned(), value)]);
    assert!(
        decision
            .resolve_options(&kinds(json!(["a", "c"])), None)
            .is_ok()
    );
    let problem = decision
        .resolve_options(&kinds(json!(["a", "z"])), None)
        .unwrap_err()
        .to_string();
    assert!(problem.contains("must be one of"), "{problem}");
}

/// Decisions `p/<name>` with the options each is given.
fn catalog(decisions: &[(&str, &str)]) -> Catalog {
    let names: Vec<&str> = decisions.iter().map(|(name, _)| *name).collect();
    let mut all = files(&[("p/pack.yaml", pack("p", &[("s", &names)]))]);
    for (name, options) in decisions {
        let text = decision(
            &format!("p/{name}"),
            &format!("{SPEC}  severity: warn\n{options}"),
        );
        all.insert(format!("p/s/{name}.yaml"), text);
    }
    Catalog::from_files(all, &lighthouse_test_support::cel()).unwrap()
}

fn resolve(catalog: &Catalog, project: &str) -> Arc<Rules> {
    Config::parse_inline(&format!("extends = [\"p/{project}\"]"))
        .unwrap()
        .resolve(Path::new("a.go"), "go", catalog.projects())
        .unwrap()
}

fn plain() -> String {
    CHECK.to_owned()
}

#[test]
fn strict_enables_every_kind_of_a_decision_that_enables_some_by_default() {
    let narrow = with_kinds(NARROW);
    let catalog = catalog(&[("a", &narrow), ("b", &plain())]);

    let recommended = resolve(&catalog, "recommended");
    assert!(recommended["p/a"].options.is_empty());
    let strict = resolve(&catalog, "strict");
    assert_eq!(strict["p/a"].level, Some(Severity::Warn));
    assert_eq!(strict["p/a"].options["kinds"], json!(["a", "b", "c"]));
    assert!(strict["p/b"].options.is_empty());
}

#[test]
fn a_decision_that_enables_all_its_kinds_by_default_is_left_alone() {
    let all = with_kinds(
        "        type: array\n        items: { enum: [a, b] }\n        default: [a, b]\n",
    );
    let catalog = catalog(&[("a", &all)]);

    assert!(!catalog.projects().names().any(|n| n == "p/strict"));
}

#[test]
fn a_pack_without_narrow_kinds_has_no_strict_project() {
    let catalog = catalog(&[("a", &plain())]);

    let projects = catalog.projects();
    let names: Vec<&str> = projects.names().collect();
    assert_eq!(names, ["p/recommended"]);
}
