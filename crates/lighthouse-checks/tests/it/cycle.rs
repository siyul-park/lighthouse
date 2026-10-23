//! The `cycle` field of a module: the strongly connected component of the
//! import graph that holds it, read by an ordinary CEL check.

use std::collections::BTreeMap;

use crate::support::{Subject, World};
use lighthouse_checks::Checks;
use lighthouse_model::ucm::EdgeKind;
use lighthouse_spec::Catalog;
use serde_json::json;

/// One finding per cycle, at its first module.
fn plugin() -> Checks {
    let text = "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/acyclic\nspec:\n  title: No cycles\n  context: Dependencies point one way.\n  scope: { subject: project }\n  requirement: Modules MUST NOT depend on each other in a circle.\n  severity: error\n  check:\n    type: cel\n    select: module\n    where: \"size(module.cycle) > 0 && module.cycle[0] == module.path\"\n    message: \"dependency cycle of {{ size(module.cycle) }} modules over imports edges: {{ module.cycle.join(' -> ') }}\"\n    identity:\n      subject: \"module.cycle.join(',')\"\n  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{ path: a.txt, body: x }]\n      expect: [{ line: 1 }]\n    - name: good\n      language: text\n      kind: valid\n      files: [{ path: a.txt, body: y }]\n".to_owned();
    let layer = Catalog::from_local(BTreeMap::from([("acyclic.yaml".to_owned(), text)])).unwrap();
    Checks::new("local", &layer).unwrap()
}

fn found(plugin: &Checks, w: &World) -> Vec<(String, u32)> {
    w.check_in(
        &Subject {
            plugin,
            id: "local",
        },
        "local/acyclic",
        json!({}),
    )
}

#[test]
fn a_cycle_of_modules_is_one_finding_and_a_line_of_modules_is_none() {
    let mut w = World::default();
    for path in ["a", "b", "c"] {
        w.module(path, None, None);
    }
    let a = w.func("a", "A", "a/a.ucm");
    let b = w.func("b", "B", "b/b.ucm");
    let c = w.func("c", "C", "c/c.ucm");
    w.edge(EdgeKind::Imports, &a, &b);
    w.edge(EdgeKind::Imports, &b, &a);
    w.edge(EdgeKind::Imports, &b, &c);

    let result = found(&plugin(), &w);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "a/a.ucm");
}

#[test]
fn a_long_chain_is_walked_without_recursion() {
    let mut w = World::default();
    w.module("m0", None, None);
    let mut previous = w.func("m0", "F", "m0/f.ucm");
    for n in 1..1500 {
        w.module(&format!("m{n}"), None, None);
        let next = w.func(&format!("m{n}"), "F", &format!("m{n}/f.ucm"));
        w.edge(EdgeKind::Imports, &previous, &next);
        previous = next;
    }

    assert!(found(&plugin(), &w).is_empty());
}
