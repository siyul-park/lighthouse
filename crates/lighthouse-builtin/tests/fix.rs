//! The fix of a bundled-style pack that fixes by command: examples run it only
//! when the configuration allows commands of other catalogs.

use lighthouse_declarative::Declarative;
use lighthouse_engine::RuleTester;
use lighthouse_plugin::Registry;
use lighthouse_spec::Catalog;

const DECISION: &str = r#"apiVersion: lighthouse/v1alpha1
kind: Decision
metadata:
  name: demo/shout
  labels:
    lighthouse/pack: demo
    lighthouse/section: s
spec:
  title: Scratch notes are emptied
  intent: A scratch note holds nothing.
  scope: { subject: file }
  requirement: A scratch note MUST be empty.
  severity: error
  evidence: [path]
  check:
    type: cel
    select: file
    where: 'file.path == "a.txt" && file.lines > 0'
    message: not empty
  fix:
    safety: safe
    type: command
    argv: ["sh", "-c", ": > \"$1\"", "sh", "{file}"]
    output: in-place
  examples:
    - name: quiet
      language: text
      kind: invalid
      files:
        - path: a.txt
          body: hello
      expect:
        - line: 1
      fixed:
        - path: a.txt
          body: ""
    - name: empty
      language: text
      kind: valid
      files:
        - path: a.txt
          body: ""
"#;

const PACK: &str = "apiVersion: lighthouse/v1alpha1\nkind: Pack\nmetadata:\n  name: demo\nspec:\n  title: Demo\n  intro: x\n  sections:\n    - name: s\n      title: S\n      intro: x\n      decisions: [shout]\n";

fn catalog() -> Catalog {
    let files = [("demo/pack.yaml", PACK), ("demo/s/shout.yaml", DECISION)]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
    Catalog::from_files(files).unwrap()
}

fn registry() -> Registry {
    let mut registry = lighthouse_builtin::registry();
    registry
        .register(&Declarative::from_catalog("demo", &catalog()).unwrap())
        .unwrap();
    registry
}

#[test]
fn a_command_does_not_run_in_examples_until_the_project_is_trusted() {
    let catalog = catalog();

    let denied = RuleTester::new(registry, &catalog)
        .trusted(false)
        .check_all();
    let allowed = RuleTester::new(registry, &catalog)
        .trusted(true)
        .check_all();

    assert_eq!(denied.len(), 1, "{denied:?}");
    assert!(denied[0].contains("lighthouse trust"), "{denied:?}");
    assert!(allowed.is_empty(), "{allowed:?}");
}
