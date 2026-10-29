//! The code of a file in several languages, such as the fences of a Markdown
//! file, is judged under the options of its own language.

use std::collections::BTreeMap;

use crate::support::{Subject, World};
use lighthouse_checks::Checks;
use lighthouse_spec::Catalog;
use serde_json::json;

const DECISION: &str = r#"
apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: local/name-length
spec:
  title: Names stay short
  context: A name that is too long is hard to read.
  scope: { subject: symbol }
  requirement: A name MUST NOT be longer than the option `max`.
  severity: warn
  options:
    properties:
      max: { type: integer, default: 100, description: The longest name. }
    additionalProperties: false
  languages:
    go: { options: { max: 3 } }
    rust: { options: { max: 6 } }
  check:
    type: cel
    where: symbol.name.size() > options.max
    message: "{{ symbol.name }}"
  examples:
  - name: long
    language: go
    kind: invalid
    files: [{ path: a.go, body: "package a\nfunc Abcd() {}\n" }]
    expect: [{ line: 2 }]
  - name: short
    language: go
    kind: valid
    canonical: true
    files: [{ path: a.go, body: "package a\nfunc Ab() {}\n" }]
  - name: long
    language: rust
    kind: invalid
    files: [{ path: a.rs, body: "fn abcdefg() {}\n" }]
    expect: [{ line: 1 }]
  - name: short
    language: rust
    kind: valid
    canonical: true
    files: [{ path: a.rs, body: "fn abcd() {}\n" }]
"#;

fn judged(world: &World) -> Vec<String> {
    let catalog = Catalog::from_local(
        BTreeMap::from([("name-length.yaml".to_owned(), DECISION.to_owned())]),
        &lighthouse_test_support::cel(),
    )
    .unwrap();
    let plugin = Checks::new(&lighthouse_test_support::composition(), "local", &catalog).unwrap();
    let mut found: Vec<String> = world
        .diagnose_with(
            &catalog,
            &Subject {
                plugin: &plugin,
                id: "local",
            },
            "local/name-length",
            json!({}),
        )
        .into_iter()
        .map(|d| d.message)
        .collect();
    found.sort();
    found
}

#[test]
fn the_symbols_of_a_file_in_two_languages_are_judged_under_the_options_of_each() {
    let mut world = World::default();
    world.module_in("go-fence", "go");
    world.module_in("rust-fence", "rust");
    // Four letters are too long for Go here, and fine for Rust.
    world.symbol_in("go-fence", "abcd", "m/doc.ucm");
    world.symbol_in("rust-fence", "efgh", "m/doc.ucm");
    // Seven letters are too long for both.
    world.symbol_in("go-fence", "ijklmno", "m/doc.ucm");
    world.symbol_in("rust-fence", "pqrstuv", "m/doc.ucm");

    assert_eq!(judged(&world), ["abcd", "ijklmno", "pqrstuv"]);
}
