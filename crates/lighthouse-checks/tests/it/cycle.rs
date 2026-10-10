//! The `cycle` operation: strongly connected components over one kind of edge.

use std::collections::BTreeMap;

use crate::support::{Subject, World};
use lighthouse_checks::Declarative;
use lighthouse_model::EdgeKind;
use lighthouse_spec::Catalog;
use serde_json::json;

fn plugin(level: &str) -> Declarative {
    let text = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/acyclic\nspec:\n  title: No cycles\n  context: Dependencies point one way.\n  scope: {{ subject: project }}\n  requirement: Modules MUST NOT depend on each other in a circle.\n  severity: error\n  check:\n    type: builtin\n    op: cycle\n    edge: imports\n    level: {level}\n  examples:\n    - name: bad\n      language: text\n      kind: invalid\n      files: [{{ path: a.txt, body: x }}]\n      expect: [{{ line: 1 }}]\n    - name: good\n      language: text\n      kind: valid\n      files: [{{ path: a.txt, body: y }}]\n"
    );
    let layer = Catalog::from_local(BTreeMap::from([("acyclic.yaml".to_owned(), text)])).unwrap();
    Declarative::from_catalog("local", &layer).unwrap()
}

fn found(plugin: &Declarative, w: &World) -> Vec<(String, u32)> {
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
    let a = w.func("a", "A", "a/a.ucm");
    let b = w.func("b", "B", "b/b.ucm");
    let c = w.func("c", "C", "c/c.ucm");
    w.edge(EdgeKind::Imports, &a, &b);
    w.edge(EdgeKind::Imports, &b, &a);
    w.edge(EdgeKind::Imports, &b, &c);

    let result = found(&plugin("module"), &w);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "a/a.ucm");
}

#[test]
fn a_long_chain_is_walked_without_recursion() {
    let mut w = World::default();
    let mut previous = w.func("m0", "F", "m0/f.ucm");
    for n in 1..4000 {
        let next = w.func(&format!("m{n}"), "F", &format!("m{n}/f.ucm"));
        w.edge(EdgeKind::Imports, &previous, &next);
        previous = next;
    }

    assert!(found(&plugin("module"), &w).is_empty());
}
