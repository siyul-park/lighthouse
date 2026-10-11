//! The decisions about where functions live: feature envy, misplaced
//! symbols over synthetic projects.

use crate::support::World;
use lighthouse_model::{EdgeKind, Symbol, SymbolKind, Visibility};
use serde_json::json;

const ENVY: &str = "design/feature-envy";
const MISPLACED: &str = "design/misplaced-symbol";

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

#[test]
fn feature_envy_reports_a_private_function_that_uses_members_of_one_type() {
    let (w, _) = envious(&["items", "total", "extra"]);
    assert_eq!(w.names(&w.check(ENVY, json!({}))), ["summary"]);
}

#[test]
fn feature_envy_needs_enough_members_and_no_other_type() {
    let (w, _) = envious(&["items"]);
    assert!(w.check(ENVY, json!({})).is_empty(), "one member");
    assert_eq!(w.check(ENVY, json!({ "minMembers": 1 })).len(), 1);

    let (mut w, summary) = envious(&["items", "total", "extra"]);
    let cache = w.symbol("m", "Cache", SymbolKind::Type, "m/a.ucm");
    let hits = w.member(&cache, "hits", SymbolKind::Field, "m/a.ucm");
    w.edge(EdgeKind::References, &summary, &hits);
    assert!(w.check(ENVY, json!({})).is_empty(), "members of two types");
}

#[test]
fn feature_envy_leaves_exported_constructors_and_used_values_alone() {
    let (mut w, summary) = envious(&["items", "total", "extra"]);
    let user = w.func("m", "run", "m/a.ucm");
    w.edge(EdgeKind::References, &user, &summary);
    assert!(w.check(ENVY, json!({})).is_empty(), "used as a value");

    let (mut w, summary) = envious(&["items", "total", "extra"]);
    w.symbols
        .iter_mut()
        .find(|s| s.id == summary.id)
        .unwrap()
        .visibility = Visibility::Public;
    assert!(w.check(ENVY, json!({})).is_empty(), "exported");
    assert_eq!(w.check(ENVY, json!({ "includeExported": true })).len(), 1);

    let (mut w, summary) = envious(&["items", "total", "extra"]);
    w.symbols
        .iter_mut()
        .find(|s| s.id == summary.id)
        .unwrap()
        .name = "newSummary".to_owned();
    assert!(
        w.check(ENVY, json!({})).is_empty(),
        "named like a constructor"
    );
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
    assert!(w.check(ENVY, json!({})).is_empty());
}

#[test]
fn misplaced_symbol_reports_a_function_that_uses_one_other_module_only() {
    let mut w = World::default();
    let load = w.func("app", "load", "app/a.ucm");
    w.summarize(&load, 4, &[]);
    w.private(&load);
    for name in ["read", "count", "scan"] {
        let used = w.func("store", name, "store/a.ucm");
        w.edge(EdgeKind::Calls, &load, &used);
    }
    assert_eq!(w.names(&w.check(MISPLACED, json!({}))), ["load"]);
    assert!(w.check(MISPLACED, json!({ "minUses": 4 })).is_empty());

    let local = w.func("app", "local", "app/a.ucm");
    w.edge(EdgeKind::Calls, &load, &local);
    assert!(
        w.check(MISPLACED, json!({})).is_empty(),
        "uses its own module"
    );
}

#[test]
fn misplaced_symbol_does_not_move_a_function_into_generated_code() {
    let mut w = World::default();
    let load = w.func("app", "load", "app/a.ucm");
    w.summarize(&load, 4, &[]);
    w.private(&load);
    for name in ["read", "count", "scan"] {
        let used = w.func("gen", name, "gen/a.ucm");
        w.edge(EdgeKind::Calls, &load, &used);
    }
    assert_eq!(w.names(&w.check(MISPLACED, json!({}))), ["load"]);
    w.files.insert("gen/a.ucm".to_owned(), true);
    assert!(w.check(MISPLACED, json!({})).is_empty());
}

#[test]
fn misplaced_symbol_leaves_a_function_that_uses_two_other_modules_alone() {
    let mut w = World::default();
    let load = w.func("app", "load", "app/a.ucm");
    w.summarize(&load, 4, &[]);
    w.private(&load);
    for (module, name) in [("store", "read"), ("log", "write")] {
        let used = w.func(module, name, &format!("{module}/a.ucm"));
        w.edge(EdgeKind::Calls, &load, &used);
    }
    assert!(w.check(MISPLACED, json!({})).is_empty());
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
    assert!(w.check(ENVY, json!({})).is_empty());
}

#[test]
fn feature_envy_and_misplaced_symbol_skip_short_functions() {
    let (mut w, summary) = envious(&["items", "total", "extra"]);
    w.summarize(&summary, 2, &[]);
    assert!(w.check(ENVY, json!({})).is_empty());
    assert_eq!(w.check(ENVY, json!({ "minStatements": 2 })).len(), 1);

    let mut w = World::default();
    let load = w.func("app", "load", "app/a.ucm");
    w.private(&load);
    for name in ["read", "count", "scan"] {
        let used = w.func("store", name, "store/a.ucm");
        w.edge(EdgeKind::Calls, &load, &used);
    }
    assert!(w.check(MISPLACED, json!({})).is_empty());
    assert_eq!(w.check(MISPLACED, json!({ "minStatements": 1 })).len(), 1);
}

#[test]
fn feature_envy_and_misplaced_symbol_skip_entry_points_and_wiring() {
    let (mut w, _) = envious(&["items", "total", "extra"]);
    w.modules.clear();
    w.module("m", Some("main"), None);
    assert!(w.check(ENVY, json!({})).is_empty(), "package main");

    let (mut w, summary) = envious(&["items", "total", "extra"]);
    let table = w.symbol("m", "handlers", SymbolKind::Var, "m/a.ucm");
    w.edge(EdgeKind::Calls, &table, &summary);
    assert!(
        w.check(ENVY, json!({})).is_empty(),
        "a variable initializer"
    );

    let (mut w, summary) = envious(&["items", "total", "extra"]);
    let init = w.func("m", "init", "m/a.ucm");
    w.edge(EdgeKind::Calls, &init, &summary);
    assert!(w.check(ENVY, json!({})).is_empty(), "init");
}
