use std::collections::{BTreeMap, BTreeSet};

use lighthouse_spec::Catalog;
use lighthouse_test_support::{bundled, cel};

use super::{
    Conflicts, Found, Gaps, Nogood, NogoodKind, declared::candidates, proven::incomplete_entries,
    resolved,
};

const FIRST: &str = "design/declaration-groups";
const SECOND: &str = "design/top-down-order";

fn nogood<'c>(catalog: &'c Catalog, ids: &[&str], kind: NogoodKind) -> Nogood<'c> {
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
    let pair = nogood(catalog, &[FIRST, SECOND], NogoodKind::Order);

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
    let pair = nogood(catalog, &[FIRST, SECOND], NogoodKind::Order);

    assert!(pair.entries(catalog, &BTreeSet::new()).is_empty());
}

#[test]
fn a_decision_that_flags_every_order_alone_is_an_order_nogood_with_itself() {
    let catalog = bundled();
    let single = nogood(catalog, &[FIRST], NogoodKind::Order);

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

/// A local decision that declares, on its valid example, the conflicts given.
fn declaring(name: &str, conflicts: &str, extra: &str) -> String {
    format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/{name}\nspec:\n  title: T\n  context: C\n  scope: {{ subject: file }}\n  requirement: R MUST hold.\n  severity: warn\n{extra}  check:\n    type: cel\n    select: file\n    where: 'true'\n    message: m\n  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{{ path: a.txt, body: x }}]\n    - name: good\n      language: text\n      kind: valid\n      files: [{{ path: a.txt, body: x }}]\n      conflicts: [{conflicts}]\n"
    )
}

fn project(files: [(&str, String); 2]) -> (Conflicts, Vec<lighthouse_model::Document>) {
    let documents = files
        .iter()
        .flat_map(|(name, text)| {
            let path = std::path::Path::new(name);
            let format = lighthouse_model::Format::of_path(path).unwrap();
            lighthouse_model::Document::list(format, path, text).0
        })
        .collect();
    let local = Catalog::from_local(
        files.map(|(name, text)| (name.to_owned(), text)).into(),
        &cel(),
    )
    .unwrap();
    (
        Conflicts::new(std::sync::Arc::new(local), std::sync::Arc::new(|_| true)),
        documents,
    )
}

#[test]
fn a_conflict_an_example_declares_is_an_empirical_candidate_of_its_decision() {
    let (conflicts, documents) = project([
        ("a.yaml", declaring("a", "local/b", "")),
        ("b.yaml", declaring("b", "", "")),
    ]);

    let found = candidates(&conflicts, &documents, &BTreeMap::new());

    let key = lighthouse_plugin::Documents::key(&documents[0]);
    assert_eq!(found[key].len(), 1);
    assert_eq!(found[key][0]["with"], serde_json::json!(["local/b"]));
    assert_eq!(found[key][0]["basis"], "empirical");
    assert_eq!(found[key][0]["resolved"], false);
    let other = lighthouse_plugin::Documents::key(&documents[1]);
    assert!(found[other].is_empty());
}

#[test]
fn a_candidate_that_a_search_proved_is_left_to_the_error_decision() {
    let (conflicts, documents) = project([
        ("a.yaml", declaring("a", "local/b", "")),
        ("b.yaml", declaring("b", "", "")),
    ]);
    let key = lighthouse_plugin::Documents::key(&documents[0]).to_owned();
    let proven = BTreeMap::from([(
        key.clone(),
        vec![serde_json::json!({ "with": ["local/b"], "basis": "proven" })],
    )]);

    let found = candidates(&conflicts, &documents, &proven);

    assert!(found[&key].is_empty());
}

#[test]
fn a_candidate_that_one_decision_settles_by_superseding_the_other_is_resolved() {
    let (conflicts, documents) = project([
        (
            "a.yaml",
            declaring("a", "local/b", "  supersedes: [local/b]\n"),
        ),
        ("b.yaml", declaring("b", "", "  status: superseded\n")),
    ]);

    let found = candidates(&conflicts, &documents, &BTreeMap::new());

    let key = lighthouse_plugin::Documents::key(&documents[0]);
    assert_eq!(found[key][0]["resolved"], true);
}

fn everything() -> Conflicts {
    Conflicts::new(
        std::sync::Arc::new(bundled().clone()),
        std::sync::Arc::new(|_| true),
    )
}

#[test]
fn found() {
    let found = Found {
        nogoods: vec![nogood(bundled(), &[FIRST, SECOND], NogoodKind::Order)],
        lost: Vec::new(),
        incomplete: Vec::new(),
        bounded: None,
    };

    assert_eq!(found.nogoods.len(), 1);
    assert!(found.lost_notice("go").is_none());
}

#[test]
fn conflicts_catalog() {
    assert!(everything().catalog().decision(FIRST).is_some());
}

#[test]
fn conflicts_orders() {
    let only_first = |decision: &lighthouse_spec::Decision| decision.id() == FIRST;
    let found = Conflicts::new(
        std::sync::Arc::new(bundled().clone()),
        std::sync::Arc::new(only_first),
    );

    assert!(found.orders(bundled().decision(FIRST).unwrap()));
    assert!(!found.orders(bundled().decision(SECOND).unwrap()));
}

#[test]
fn conflicts_search_once() {
    let first = bundled().decision(FIRST).unwrap();
    let registry = std::sync::Arc::new(lighthouse_plugin::Registry::default());

    let conflicts = everything();
    let found = conflicts.search_once(
        &registry,
        "text",
        vec![(first, lighthouse_model::Options::new())],
    );

    assert!(found.nogoods.is_empty());
    assert!(
        found.incomplete[0].contains("no renderer"),
        "{:?}",
        found.incomplete
    );
}

#[test]
fn found_lost_notice() {
    let found = Found {
        nogoods: Vec::new(),
        lost: vec!["key".to_owned()],
        incomplete: vec!["no renderer".to_owned()],
        bounded: None,
    };

    let told = found.lost_notice("go").unwrap();

    assert!(told.contains("go decisions names decisions the catalog does not hold"));
    assert!(
        Found {
            lost: Vec::new(),
            ..found
        }
        .lost_notice("go")
        .is_none()
    );
}

#[test]
fn gaps() {
    assert!(Gaps::default().notice().is_none());
}

#[test]
fn gaps_add() {
    let mut gaps = Gaps::default();
    let found = |why: &str| Found {
        nogoods: Vec::new(),
        lost: Vec::new(),
        incomplete: vec![why.to_owned()],
        bounded: None,
    };

    gaps.add("go", &found("too small"));
    gaps.add("go", &found("too small"));
    gaps.add("python", &found("no renderer"));
    gaps.add(
        "rust",
        &Found {
            incomplete: Vec::new(),
            ..found("")
        },
    );

    assert_eq!(
        gaps.notice().unwrap(),
        "the search for contradictions is incomplete: go: too small | python: no renderer"
    );
}

#[test]
fn nogood_resolved() {
    let pair = nogood(bundled(), &[FIRST, SECOND], NogoodKind::Order);

    assert!(!pair.resolved(bundled()));
}

#[test]
fn nogood_resolver() {
    let pair = nogood(bundled(), &[FIRST, SECOND], NogoodKind::Order);

    assert_eq!(pair.resolver(bundled()), None);
}

#[test]
fn nogood_names() {
    let pair = nogood(bundled(), &[SECOND, FIRST], NogoodKind::Order);

    assert_eq!(pair.names(), [FIRST, SECOND]);
}

#[test]
fn a_decision_is_told_by_its_id_and_by_its_key_only_when_it_has_no_id() {
    let conflicts = everything();
    let first = bundled().decision(FIRST).unwrap();

    assert_eq!(conflicts.id_of(first.key()), FIRST);
    assert_eq!(conflicts.id_of("unknown-key"), "unknown-key");
}
