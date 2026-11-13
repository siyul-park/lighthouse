use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_model::{Document, Position};
use lighthouse_plugin::{Documents, NO_DOCUMENTS};
use serde_json::json;

fn decision(name: &str, uid: Option<&str>) -> Document {
    Document {
        file: PathBuf::from("a.yaml"),
        at: Position { line: 1, col: 1 },
        kind: "Decision".to_owned(),
        name: name.to_owned(),
        uid: uid.map(str::to_owned),
        labels: BTreeMap::new(),
    }
}

#[test]
fn documents() {
    let found = Documents::new(vec![decision("a/b", None)], BTreeMap::new());

    assert_eq!(found.list().len(), 1);
    assert!(NO_DOCUMENTS.list().is_empty());
}

#[test]
fn documents_new_keeps_the_facts_of_each_document_by_its_key() {
    let first = decision("a/b", Some("uid-1"));
    let second = decision("a/c", None);
    let facts = BTreeMap::from([
        ("uid-1".to_owned(), json!({ "conflicts": [1] })),
        ("a/c".to_owned(), json!({ "conflicts": [] })),
    ]);

    let found = Documents::new(vec![first.clone(), second.clone()], facts);

    assert_eq!(found.facts_of(&first), Some(&json!({ "conflicts": [1] })));
    assert_eq!(found.facts_of(&second), Some(&json!({ "conflicts": [] })));
}

#[test]
fn documents_none_has_no_document_and_no_facts() {
    let none = Documents::none();

    assert!(none.list().is_empty());
    assert_eq!(none.facts_of(&decision("a/b", None)), None);
}

#[test]
fn documents_list_is_in_the_order_given() {
    let found = Documents::new(
        vec![decision("a/z", None), decision("a/a", None)],
        BTreeMap::new(),
    );

    let names: Vec<&str> = found.list().iter().map(|d| d.name.as_str()).collect();

    assert_eq!(names, ["a/z", "a/a"]);
}

#[test]
fn documents_facts_of_is_none_for_a_document_without_facts() {
    let found = Documents::new(vec![decision("a/b", None)], BTreeMap::new());

    assert_eq!(found.facts_of(&decision("a/b", None)), None);
}

#[test]
fn documents_key_is_the_uid_else_the_name() {
    assert_eq!(Documents::key(&decision("a/b", Some("uid-1"))), "uid-1");
    assert_eq!(Documents::key(&decision("a/b", None)), "a/b");
}
