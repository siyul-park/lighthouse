//! The decisions about where functions live: feature envy and misplaced
//! symbols, the `envy` and `module` kinds of `design/prefer-method`. Both are
//! review candidates, so the tests read the `review` of the check.

use std::collections::BTreeMap;

use crate::support::{Subject, World};
use lighthouse_checks::Declarative;
use lighthouse_model::{EdgeKind, Symbol, SymbolKind, Visibility};
use lighthouse_spec::Catalog;
use serde_json::{Value, json};

const RULE: &str = "design/prefer-method";

/// The limits of the kind `kind` (`envy` or `module`) as `overrides` change
/// them from the defaults of the decision.
fn limits(kind: &str, overrides: Value) -> Value {
    let mut base = if kind == "envy" {
        json!({ "minMembers": 3, "minStatements": 3, "includeExported": false })
    } else {
        json!({ "minUses": 3, "minStatements": 3, "includeExported": false })
    };
    for (key, value) in overrides.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

/// The `(file, line)` of what the review of `design/prefer-method` offers a
/// model for the kind `kind`: the same check with its `review` as `where`.
fn candidates(world: &World, kind: &str, overrides: Value) -> Vec<(String, u32)> {
    let mut spec = serde_json::to_value(Catalog::bundled().decision(RULE).unwrap().spec()).unwrap();
    let review = spec["check"]["review"].take();
    spec["check"]["where"] = review;
    spec["check"].as_object_mut().unwrap().remove("review");
    spec["examples"] = json!([
        { "name": "bad", "language": "text", "kind": "invalid",
          "files": [{ "path": "a.txt", "body": "x" }], "expect": [{ "line": 1 }] },
        { "name": "good", "language": "text", "kind": "valid",
          "files": [{ "path": "a.txt", "body": "x" }] },
    ]);
    let text = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/review\nspec: {spec}\n"
    );
    let catalog = Catalog::from_local(BTreeMap::from([("review.yaml".to_owned(), text)])).unwrap();
    let plugin = Declarative::from_catalog("local", &catalog).unwrap();
    let options = json!({ "kinds": [kind], kind: limits(kind, overrides) });
    world
        .diagnose_with(
            &catalog,
            &Subject {
                plugin: &plugin,
                id: "local",
            },
            "local/review",
            options,
        )
        .into_iter()
        .map(|d| (d.file.to_string_lossy().into_owned(), d.span.start.line))
        .collect()
}

fn envy(world: &World, overrides: Value) -> Vec<(String, u32)> {
    candidates(world, "envy", overrides)
}

fn misplaced(world: &World, overrides: Value) -> Vec<(String, u32)> {
    candidates(world, "module", overrides)
}

/// A type `Store` with fields `items`, `total` and `extra` in module `m`, and a
/// private function `summary` of four statements that references the fields in
/// `uses`.
fn envious(uses: &[&str]) -> (World, Symbol) {
    let mut w = World::default();
    let store = w.symbol("m", "Store", SymbolKind::Type, "m/a.ucm");
    let summary = w.func("m", "summary", "m/a.ucm");
    w.summarize(&summary, 4, &[]);
    w.private(&summary);
    for name in ["items", "total", "extra"] {
        let field = w.member(&store, name, SymbolKind::Field, "m/a.ucm");
        if uses.contains(&name) {
            w.edge(EdgeKind::References, &summary, &field);
        }
    }
    (w, summary)
}

/// A private function `load` of module `app` that calls `names`, one function
/// each, of module `module`.
fn loader(module: &str, names: &[&str]) -> (World, Symbol) {
    let mut w = World::default();
    let load = w.func("app", "load", "app/a.ucm");
    w.summarize(&load, 4, &[]);
    w.private(&load);
    for name in names {
        let used = w.func(module, name, &format!("{module}/a.ucm"));
        w.edge(EdgeKind::Calls, &load, &used);
    }
    (w, load)
}

#[test]
fn feature_envy_offers_a_private_function_that_uses_members_of_one_type() {
    let (w, _) = envious(&["items", "total", "extra"]);
    assert_eq!(w.names(&envy(&w, json!({}))), ["summary"]);
    assert!(w.check(RULE, json!({})).is_empty(), "no finding, a review");
}

#[test]
fn feature_envy_needs_enough_members_and_no_other_type() {
    let (w, _) = envious(&["items"]);
    assert!(envy(&w, json!({})).is_empty(), "one member");
    assert_eq!(envy(&w, json!({ "minMembers": 1 })).len(), 1);

    let (mut w, summary) = envious(&["items", "total", "extra"]);
    let cache = w.symbol("m", "Cache", SymbolKind::Type, "m/a.ucm");
    let hits = w.member(&cache, "hits", SymbolKind::Field, "m/a.ucm");
    w.edge(EdgeKind::References, &summary, &hits);
    assert!(envy(&w, json!({})).is_empty(), "members of two types");
}

#[test]
fn feature_envy_leaves_exported_constructors_and_used_values_alone() {
    let (mut w, summary) = envious(&["items", "total", "extra"]);
    let user = w.func("m", "run", "m/a.ucm");
    w.edge(EdgeKind::References, &user, &summary);
    assert!(envy(&w, json!({})).is_empty(), "used as a value");

    let (mut w, summary) = envious(&["items", "total", "extra"]);
    w.symbols
        .iter_mut()
        .find(|s| s.id == summary.id)
        .unwrap()
        .visibility = Visibility::Public;
    assert!(envy(&w, json!({})).is_empty(), "exported");
    assert_eq!(envy(&w, json!({ "includeExported": true })).len(), 1);

    let (mut w, summary) = envious(&["items", "total", "extra"]);
    w.symbols
        .iter_mut()
        .find(|s| s.id == summary.id)
        .unwrap()
        .name = "newSummary".to_owned();
    assert!(envy(&w, json!({})).is_empty(), "named like a constructor");
}

#[test]
fn feature_envy_leaves_a_function_of_another_module_alone() {
    let mut w = World::default();
    let store = w.symbol("a", "Store", SymbolKind::Type, "a/a.ucm");
    let summary = w.func("b", "summary", "b/a.ucm");
    w.summarize(&summary, 4, &[]);
    w.private(&summary);
    for name in ["items", "total", "extra"] {
        let field = w.member(&store, name, SymbolKind::Field, "a/a.ucm");
        w.edge(EdgeKind::References, &summary, &field);
    }
    assert!(envy(&w, json!({})).is_empty());
}

#[test]
fn misplaced_symbol_offers_a_function_that_uses_one_other_module_only() {
    let (mut w, load) = loader("store", &["read", "count", "scan"]);
    assert_eq!(w.names(&misplaced(&w, json!({}))), ["load"]);
    assert!(misplaced(&w, json!({ "minUses": 4 })).is_empty());

    let local = w.func("app", "local", "app/a.ucm");
    w.edge(EdgeKind::Calls, &load, &local);
    assert!(misplaced(&w, json!({})).is_empty(), "uses its own module");
}

#[test]
fn misplaced_symbol_does_not_move_a_function_into_generated_code() {
    let (mut w, _) = loader("gen", &["read", "count", "scan"]);
    assert_eq!(w.names(&misplaced(&w, json!({}))), ["load"]);
    w.files.insert("gen/a.ucm".to_owned(), true);
    assert!(misplaced(&w, json!({})).is_empty());
}

#[test]
fn misplaced_symbol_leaves_a_function_that_uses_two_other_modules_alone() {
    let (mut w, load) = loader("store", &["read"]);
    let used = w.func("log", "write", "log/a.ucm");
    w.edge(EdgeKind::Calls, &load, &used);
    assert!(misplaced(&w, json!({})).is_empty());
}

#[test]
fn feature_envy_does_not_count_the_variants_of_an_enum() {
    let mut w = World::default();
    let level = w.symbol("m", "Level", SymbolKind::Type, "m/a.ucm");
    let rank = w.func("m", "rank", "m/a.ucm");
    w.summarize(&rank, 4, &[]);
    w.private(&rank);
    for name in ["Error", "Warn", "Info"] {
        let variant = w.member(&level, name, SymbolKind::Variant, "m/a.ucm");
        w.edge(EdgeKind::References, &rank, &variant);
    }
    assert!(envy(&w, json!({})).is_empty());
}

#[test]
fn feature_envy_and_misplaced_symbol_skip_short_functions() {
    let (mut w, summary) = envious(&["items", "total", "extra"]);
    w.summarize(&summary, 2, &[]);
    assert!(envy(&w, json!({})).is_empty());
    assert_eq!(envy(&w, json!({ "minStatements": 2 })).len(), 1);

    let (mut w, load) = loader("store", &["read", "count", "scan"]);
    w.summarize(&load, 2, &[]);
    assert!(misplaced(&w, json!({})).is_empty());
    assert_eq!(misplaced(&w, json!({ "minStatements": 2 })).len(), 1);
}

#[test]
fn feature_envy_and_misplaced_symbol_skip_entry_points_and_wiring() {
    let (mut w, _) = envious(&["items", "total", "extra"]);
    w.modules.clear();
    w.module("m", Some("main"), None);
    assert!(envy(&w, json!({})).is_empty(), "package main");

    let (mut w, summary) = envious(&["items", "total", "extra"]);
    let table = w.symbol("m", "handlers", SymbolKind::Var, "m/a.ucm");
    w.edge(EdgeKind::Calls, &table, &summary);
    assert!(envy(&w, json!({})).is_empty(), "a variable initializer");

    let (mut w, summary) = envious(&["items", "total", "extra"]);
    let init = w.func("m", "init", "m/a.ucm");
    w.edge(EdgeKind::Calls, &init, &summary);
    assert!(envy(&w, json!({})).is_empty(), "init");
}
