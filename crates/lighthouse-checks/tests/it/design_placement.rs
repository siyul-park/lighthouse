//! The decisions about where functions live: feature envy, misplaced
//! symbols and the file of an owner, over synthetic projects.

use crate::support::World;
use lighthouse_model::{EdgeKind, Symbol, SymbolKind, Visibility};
use serde_json::json;

const ENVY: &str = "design/feature-envy";
const MISPLACED: &str = "design/misplaced-symbol";
const OWNER_FILE: &str = "design/owner-file";

/// A type `Store` with fields `items` and `total` in module `m`, and a private
/// function `summary` that references the fields in `uses`.
fn envious(uses: &[&str]) -> (World, Symbol) {
    let mut w = World::default();
    let store = w.symbol("m", "Store", SymbolKind::Type, "m/a.ucm");
    let summary = w.func("m", "summary", "m/a.ucm");
    w.private(&summary);
    for name in ["items", "total"] {
        let field = w.member(&store, name, SymbolKind::Field, "m/a.ucm");
        if uses.contains(&name) {
            w.edge(EdgeKind::References, &summary, &field);
        }
    }
    (w, summary)
}

#[test]
fn feature_envy_reports_a_private_function_that_uses_members_of_one_type() {
    let (w, _) = envious(&["items", "total"]);
    assert_eq!(w.names(&w.check(ENVY, json!({}))), ["summary"]);
}

#[test]
fn feature_envy_needs_enough_members_and_no_other_type() {
    let (w, _) = envious(&["items"]);
    assert!(w.check(ENVY, json!({})).is_empty(), "one member");
    assert_eq!(w.check(ENVY, json!({ "minMembers": 1 })).len(), 1);

    let (mut w, summary) = envious(&["items", "total"]);
    let cache = w.symbol("m", "Cache", SymbolKind::Type, "m/a.ucm");
    let hits = w.member(&cache, "hits", SymbolKind::Field, "m/a.ucm");
    w.edge(EdgeKind::References, &summary, &hits);
    assert!(w.check(ENVY, json!({})).is_empty(), "members of two types");
}

#[test]
fn feature_envy_leaves_exported_constructors_and_used_values_alone() {
    let (mut w, summary) = envious(&["items", "total"]);
    let user = w.func("m", "run", "m/a.ucm");
    w.edge(EdgeKind::References, &user, &summary);
    assert!(w.check(ENVY, json!({})).is_empty(), "used as a value");

    let (mut w, summary) = envious(&["items", "total"]);
    w.symbols
        .iter_mut()
        .find(|s| s.id == summary.id)
        .unwrap()
        .visibility = Visibility::Public;
    assert!(w.check(ENVY, json!({})).is_empty(), "exported");
    assert_eq!(w.check(ENVY, json!({ "includeExported": true })).len(), 1);

    let (w, _) = envious(&["items", "total"]);
    assert!(
        w.check(ENVY, json!({ "constructorPrefixes": ["summary"] }))
            .is_empty(),
        "named like a constructor"
    );
}

#[test]
fn feature_envy_leaves_a_function_of_another_module_alone() {
    let mut w = World::default();
    let store = w.symbol("a", "Store", SymbolKind::Type, "a/a.ucm");
    let summary = w.func("b", "summary", "b/a.ucm");
    w.private(&summary);
    for name in ["items", "total"] {
        let field = w.member(&store, name, SymbolKind::Field, "a/a.ucm");
        w.edge(EdgeKind::References, &summary, &field);
    }
    assert!(w.check(ENVY, json!({})).is_empty());
}

#[test]
fn misplaced_symbol_reports_a_function_that_uses_one_other_module_only() {
    let mut w = World::default();
    let load = w.func("app", "load", "app/a.ucm");
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
fn misplaced_symbol_leaves_a_function_that_uses_two_other_modules_alone() {
    let mut w = World::default();
    let load = w.func("app", "load", "app/a.ucm");
    w.private(&load);
    for (module, name) in [("store", "read"), ("log", "write")] {
        let used = w.func(module, name, &format!("{module}/a.ucm"));
        w.edge(EdgeKind::Calls, &load, &used);
    }
    assert!(w.check(MISPLACED, json!({})).is_empty());
}

#[test]
fn owner_file_reports_a_method_declared_away_from_its_type() {
    let mut w = World::default();
    let store = w.symbol("m", "Store", SymbolKind::Type, "m/store.ucm");
    w.member(&store, "Near", SymbolKind::Method, "m/store.ucm");
    w.member(&store, "Far", SymbolKind::Method, "m/total.ucm");
    assert_eq!(w.names(&w.check(OWNER_FILE, json!({}))), ["Far"]);
}

#[test]
fn owner_file_reports_a_helper_only_methods_of_one_type_call() {
    let mut w = World::default();
    let store = w.symbol("m", "Store", SymbolKind::Type, "m/store.ucm");
    let total = w.member(&store, "Total", SymbolKind::Method, "m/store.ucm");
    let count = w.func("m", "count", "m/count.ucm");
    w.private(&count);
    w.edge(EdgeKind::Calls, &total, &count);
    assert!(
        w.check(OWNER_FILE, json!({})).is_empty(),
        "a helper that does not use the type"
    );
    w.edge(EdgeKind::References, &count, &store);
    assert_eq!(w.names(&w.check(OWNER_FILE, json!({}))), ["count"]);

    let other = w.func("m", "Other", "m/store.ucm");
    w.edge(EdgeKind::Calls, &other, &count);
    assert!(w.check(OWNER_FILE, json!({})).is_empty(), "another caller");
}
