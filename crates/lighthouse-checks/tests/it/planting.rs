use std::collections::BTreeMap;

use lighthouse_checks::with_local as plant;
use lighthouse_plugin::Registry;
use lighthouse_spec::Catalog;
use lighthouse_test_support::composition;

const HEAD: &str = "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/probe\nspec:\n  title: T\n  context: C\n  scope: { subject: file }\n  requirement: R MUST hold.\n";

const CHECKED: &str = "  severity: warn
  check:
    type: cel
    select: file
    where: 'true'
    message: m
  examples:
    - name: bad
      language: text
      kind: invalid
      files: [{ path: a.txt, body: x }]
    - name: good
      language: text
      kind: valid
      files: [{ path: a.txt, body: x }]
";

fn local(rest: &str) -> Catalog {
    let files = BTreeMap::from([("probe.yaml".to_owned(), format!("{HEAD}{rest}"))]);
    Catalog::from_local(files, composition().cel()).unwrap()
}

#[test]
fn with_local() {
    let mut registry = Registry::default();

    plant(&composition(), &mut registry, &local(CHECKED)).unwrap();

    assert!(registry.has_plugin("local"));
}

#[test]
fn with_local_registers_nothing_for_decisions_without_a_check() {
    let mut registry = Registry::default();

    plant(&composition(), &mut registry, &local("")).unwrap();

    assert!(!registry.has_plugin("local"));
}
