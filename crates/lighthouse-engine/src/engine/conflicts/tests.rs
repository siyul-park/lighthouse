use std::collections::{BTreeMap, BTreeSet};

use lighthouse_spec::Catalog;
use lighthouse_test_support::{bundled, cel};

use super::{Conflicts, Nogood, incomplete_entries, resolved};

const FIRST: &str = "design/declaration-groups";
const SECOND: &str = "design/top-down-order";

fn nogood<'c>(catalog: &'c Catalog, ids: &[&str], kind: &'static str) -> Nogood<'c> {
    Nogood {
        members: ids.iter().map(|id| catalog.decision(id).unwrap()).collect(),
        kind,
        witness: serde_json::json!({ "shape": "a public type A" }),
    }
}

fn own<'c>(catalog: &'c Catalog, ids: &[&str]) -> BTreeSet<&'c str> {
    ids.iter()
        .map(|id| catalog.decision(id).unwrap().key())
        .collect()
}

#[test]
fn a_pair_is_told_to_each_member_that_is_a_document_of_the_project() {
    let catalog = bundled();
    let pair = nogood(catalog, &[FIRST, SECOND], "order");

    let entries = pair.entries(catalog, &own(catalog, &[FIRST]));

    assert_eq!(entries.len(), 1);
    let (key, entry) = &entries[0];
    assert_eq!(key, catalog.decision(FIRST).unwrap().key());
    assert_eq!(entry["with"], serde_json::json!([SECOND]));
    assert_eq!(entry["kind"], "order");
    assert_eq!(entry["basis"], "proven");
    assert_eq!(entry["resolved"], false);
    assert_eq!(entry["names"], serde_json::json!([FIRST, SECOND]));
}

#[test]
fn a_nogood_of_pack_decisions_alone_is_told_to_no_document() {
    let catalog = bundled();
    let pair = nogood(catalog, &[FIRST, SECOND], "order");

    assert!(pair.entries(catalog, &BTreeSet::new()).is_empty());
}

#[test]
fn a_decision_that_flags_every_order_alone_is_an_order_nogood_with_itself() {
    let catalog = bundled();
    let single = nogood(catalog, &[FIRST], "order");

    let entries = single.entries(catalog, &own(catalog, &[FIRST]));

    assert_eq!(entries[0].1["with"], serde_json::json!([FIRST]));
    assert_eq!(entries[0].1["kind"], "order");
}

#[test]
fn a_member_that_supersedes_another_settles_the_contradiction() {
    let text = |name: &str, extra: &str| {
        format!(
            "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/{name}\nspec:\n  title: T\n  context: C\n  scope: {{ subject: file }}\n  requirement: R MUST hold.\n{extra}"
        )
    };
    let files = BTreeMap::from([
        ("old.yaml".to_owned(), text("old", "  status: superseded\n")),
        (
            "new.yaml".to_owned(),
            text("new", "  supersedes: [local/old]\n"),
        ),
        ("other.yaml".to_owned(), text("other", "")),
    ]);
    let local = Catalog::from_local(files, &cel()).unwrap();
    let get = |id| local.decision(id).unwrap();

    assert!(resolved(&local, &[get("local/new"), get("local/old")]));
    assert!(!resolved(&local, &[get("local/other"), get("local/old")]));
}

#[test]
fn conflicts() {
    let catalog = std::sync::Arc::new(bundled().clone());

    let found = Conflicts::new(catalog, std::sync::Arc::new(|_| true));

    assert!(found.catalog.decision(FIRST).is_some());
}

#[test]
fn conflicts_new_holds_the_decisions_the_given_test_accepts() {
    let catalog = std::sync::Arc::new(bundled().clone());
    let only_first = |decision: &lighthouse_spec::Decision| decision.id() == FIRST;

    let found = Conflicts::new(catalog, std::sync::Arc::new(only_first));

    let first = found.catalog.decision(FIRST).unwrap();
    let second = found.catalog.decision(SECOND).unwrap();
    assert!((found.orders)(first));
    assert!(!(found.orders)(second));
}

#[test]
fn an_incomplete_search_tells_each_own_member_that_nothing_is_proven_and_why() {
    let catalog = bundled();
    let members = [
        catalog.decision(FIRST).unwrap(),
        catalog.decision(SECOND).unwrap(),
    ];
    let reasons = vec!["the bound".to_owned()];

    let entries = incomplete_entries(&members, &own(catalog, &[FIRST]), &reasons);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, catalog.decision(FIRST).unwrap().key());
    assert_eq!(entries[0].1["basis"], "incomplete");
    assert_eq!(entries[0].1["reasons"], serde_json::json!(["the bound"]));
}
