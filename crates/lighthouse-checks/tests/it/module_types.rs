//! The `types` of a module (methods, cohesion groups) and the `implementers`
//! of an interface, read by checks over real Go and Rust sources.

use std::path::Path;

use lighthouse_checks::Declarative;
use lighthouse_engine::RuleTester;
use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Config};

const TYPES: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: demo/types
spec:
  title: A type has the expected shape
  context: The facts of the types of a module equal the options.
  scope: { subject: module }
  requirement: A type MUST have the expected shape.
  severity: warn
  options:
    properties:
      name: { type: string, default: Store, description: The type. }
      methods: { type: integer, default: 0, description: Its methods. }
      public: { type: integer, default: 0, description: Its public methods. }
      lcom4: { type: integer, default: 0, description: Its components. }
      groups: { type: array, default: [], description: Its groups. }
    additionalProperties: false
  check:
    type: cel
    select: module
    where: >-
      module.types.exists(t, t.name == options.name && t.methods == options.methods
        && t.public_methods == options.public && t.lcom4 == options.lcom4
        && t.groups == options.groups)
    message: shape
    at:
      symbol: module.types[0].id
  examples:
    - name: two-groups
      language: go
      kind: invalid
      options: { methods: 4, public: 4, lcom4: 2, groups: [[AddItem, CountItems], [AddName, CountNames]] }
      files:
        - path: example.go
          body: |-
            package sample

            type Store struct {
            	items []int
            	names []string
            }

            func (s *Store) AddItem(x int)    { s.items = append(s.items, x) }
            func (s *Store) CountItems() int  { return len(s.items) }
            func (s *Store) AddName(n string) { s.names = append(s.names, n) }
            func (s *Store) CountNames() int  { return len(s.names) }
      expect:
        - line: 3
    - name: one-group-by-a-call
      language: go
      kind: invalid
      options: { methods: 3, public: 2, lcom4: 1, groups: [[Add, Count, total]] }
      files:
        - path: example.go
          body: |-
            package sample

            type Store struct {
            	items []int
            }

            func (s *Store) Add(x int)  { s.items = append(s.items, x) }
            func (s *Store) Count() int { return s.total() }
            func (s *Store) total() int { return len(s.items) }
      expect:
        - line: 3
    - name: a-method-of-no-field-is-its-own-group
      language: go
      kind: invalid
      options: { methods: 2, public: 2, lcom4: 2, groups: [[Count], [Version]] }
      files:
        - path: example.go
          body: |-
            package sample

            type Store struct {
            	items []int
            }

            func (s *Store) Count() int    { return len(s.items) }
            func (s *Store) Version() int  { return 1 }
      expect:
        - line: 3
    - name: the-groups-differ
      language: go
      kind: valid
      options: { methods: 2, public: 2, lcom4: 1, groups: [[Count, Version]] }
      files:
        - path: example.go
          body: |-
            package sample

            type Store struct {
            	items []int
            }

            func (s *Store) Count() int    { return len(s.items) }
            func (s *Store) Version() int  { return 1 }
    - name: trait-methods-join-the-groups-but-are-not-counted
      language: rust
      kind: invalid
      options: { methods: 1, public: 1, lcom4: 1, groups: [[fmt, size]] }
      files:
        - path: src/lib.rs
          body: |-
            pub struct Store {
                items: Vec<i32>,
            }

            impl Store {
                pub fn size(&self) -> usize {
                    self.items.len()
                }
            }

            impl std::fmt::Display for Store {
                fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    write!(f, "{}", self.items.len())
                }
            }
      expect:
        - line: 1
"#;

const PACK: &str = "apiVersion: lighthouse/v1alpha1\nkind: Pack\nmetadata:\n  name: demo\nspec:\n  title: Demo\n  intro: x\n  sections:\n    - name: s\n      title: S\n      intro: x\n      decisions: [types]\n";

fn catalog() -> Catalog {
    let files = [("demo/pack.yaml", PACK), ("demo/s/types.yaml", TYPES)]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    Catalog::from_files(files).unwrap()
}

fn registry() -> Registry {
    let mut registry = lighthouse_checks::registry();
    let plugins: Vec<_> = [
        ("lang-go", lighthouse_test_support::lang_go()),
        ("lang-rust", Some(lighthouse_test_support::lang_rust())),
    ]
    .into_iter()
    .filter_map(|(id, path)| Some(format!("{{ id = \"{id}\", path = {:?} }}", path?.to_str()?)))
    .collect();
    let config = Config::parse_inline(&format!("plugins = [{}]", plugins.join(", "))).unwrap();
    lighthouse_rpc::register(&mut registry, &config, Path::new("."), &[]).unwrap();
    registry
        .register(&Declarative::from_catalog("demo", &catalog()).unwrap())
        .unwrap();
    registry
}

#[test]
fn module_types_count_methods_and_find_the_groups_that_share_nothing() {
    let failures = RuleTester::new(registry, &catalog()).check_all();
    assert!(failures.is_empty(), "{failures:?}");
}
