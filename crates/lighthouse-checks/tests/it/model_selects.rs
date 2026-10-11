//! The `select` of a model check picks the subjects a model is asked about.
//! Each select is run as the `where` of a cel check over a synthetic project:
//! what it selects is what the model runtime would ask about. (The runtime
//! itself is tested end to end in the CLI's `models` tests.)

use std::collections::BTreeMap;

use crate::support::{Subject, World};
use lighthouse_checks::Declarative;
use lighthouse_model::{EdgeKind, SymbolKind};
use lighthouse_spec::Catalog;
use serde_json::{Value, json};

const EXAMPLES: &str = "  examples:
    - name: bad
      language: text
      kind: invalid
      files: [{ path: a.txt, body: x }]
      expect: [{ line: 1 }]
    - name: good
      language: text
      kind: valid
      files: [{ path: a.txt, body: x }]
";

/// What the model decision `id` selects in `world`, sorted: the names of its
/// subjects (`symbol.name`, or `module.path` for a module decision).
fn selected(world: &World, id: &str, subject: &str) -> Vec<String> {
    let bundled = Catalog::bundled().decision(id).expect("a bundled decision");
    let spec: Value = serde_json::to_value(bundled.spec()).unwrap();
    let select = spec["check"]["select"]
        .as_str()
        .unwrap_or_else(|| panic!("{id} has a select"));
    let name = if subject == "module" {
        "module.path"
    } else {
        "symbol.name"
    };
    let text = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/select\n  labels:\n    lighthouse/pack: local\n    lighthouse/section: rules\nspec:\n  title: Select\n  context: A select.\n  scope: {{ subject: {subject} }}\n  requirement: A select MUST hold.\n  severity: warn\n  check:\n    type: cel\n    where: {}\n    message: \"{{{{ {name} }}}}\"\n{EXAMPLES}",
        serde_json::to_string(select).unwrap()
    );
    let catalog = Catalog::from_local(BTreeMap::from([("select.yaml".to_owned(), text)])).unwrap();
    let plugin = Declarative::from_catalog("local", &catalog).unwrap();
    let mut found: Vec<String> = world
        .diagnose_with(
            &catalog,
            &Subject {
                plugin: &plugin,
                id: "local",
            },
            "local/select",
            json!({}),
        )
        .into_iter()
        .map(|d| d.message)
        .collect();
    found.sort();
    found
}

#[test]
fn justified_helpers_asks_about_a_private_function_with_one_production_caller() {
    let mut w = World::default();
    let run = w.func("m", "run", "m/a.ucm");
    let other = w.func("m", "other", "m/a.ucm");
    for name in ["helper", "shared"] {
        let f = w.func("m", name, "m/a.ucm");
        w.private(&f);
        w.edge(EdgeKind::Calls, &run, &f);
    }
    let shared = w
        .symbols
        .iter()
        .find(|s| s.name == "shared")
        .unwrap()
        .clone();
    w.edge(EdgeKind::Calls, &other, &shared);
    assert_eq!(
        selected(&w, "design/justified-helpers", "symbol"),
        ["helper"]
    );
}

#[test]
fn terminology_asks_about_a_public_type_another_module_also_declares() {
    let mut w = World::default();
    w.symbol("a", "Account", SymbolKind::Type, "a/a.ucm");
    w.symbol("b", "Account", SymbolKind::Type, "b/a.ucm");
    w.symbol("a", "Invoice", SymbolKind::Type, "a/a.ucm");
    assert_eq!(
        selected(&w, "design/terminology", "symbol"),
        ["Account", "Account"]
    );
}

#[test]
fn meaningful_separation_asks_about_a_small_module_with_one_user() {
    let mut w = World::default();
    for (path, name) in [
        ("tiny", None),
        ("app", None),
        ("wide", None),
        ("cmd", Some("main")),
    ] {
        w.func(path, "f", &format!("{path}/a.ucm"));
        w.module(path, name, None);
    }
    w.func("rustbin[bin:tool]", "g", "tool/a.ucm");
    w.module("rustbin[bin:tool]", None, None);
    // `tiny` has one user; `wide` has two; `app` has none but is no entry
    // module; the `main` package and the binary target are entries.
    for (from, to) in [("app", "tiny"), ("app", "wide"), ("cmd", "wide")] {
        w.import(from, to);
    }
    assert_eq!(
        selected(&w, "design/meaningful-separation", "module"),
        ["app", "tiny"]
    );
}

#[test]
fn owner_file_asks_about_a_method_declared_away_from_its_type() {
    let mut w = World::default();
    let store = w.symbol("m", "Store", SymbolKind::Type, "m/store.ucm");
    w.member(&store, "Near", SymbolKind::Method, "m/store.ucm");
    w.member(&store, "Far", SymbolKind::Method, "m/total.ucm");
    assert_eq!(selected(&w, "design/owner-file", "symbol"), ["Far"]);
}
