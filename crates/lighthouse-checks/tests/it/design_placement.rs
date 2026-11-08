//! The decisions about where functions live: feature envy (a review candidate,
//! so these tests read the `abstain` of the check) and the placement model of
//! `design/module-placement`, `design/prefer-method` and
//! `design/file-placement`, one answer per function.

use std::collections::BTreeMap;

use crate::{
    model_selects::selected,
    support::{Subject, World},
};
use lighthouse_checks::Checks;
use lighthouse_model::ucm::{EdgeKind, Symbol, SymbolKind, TypeRef, Visibility};
use lighthouse_spec::Catalog;
use serde_json::{Value, json};

const RULE: &str = "design/feature-envy";

/// The limits of the kind `kind` (`envy` or `module`) as `overrides` change
/// them from the defaults of the decision.
fn limits(kind: &str, overrides: Value) -> Value {
    let mut base = json!({ "minMembers": 3, "minStatements": 3, "includeExported": false });
    let _ = kind;
    for (key, value) in overrides.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

/// The `(file, line)` of what the review of `design/prefer-method` offers a
/// model for the kind `kind`: the same check with its `abstain` as `where`.
fn candidates(world: &World, kind: &str, overrides: Value) -> Vec<(String, u32)> {
    let mut spec = serde_json::to_value(
        lighthouse_test_support::bundled()
            .decision(RULE)
            .unwrap()
            .spec(),
    )
    .unwrap();
    let review = spec["check"]["abstain"].take();
    spec["check"]["where"] = review;
    spec["check"].as_object_mut().unwrap().remove("abstain");
    spec["examples"] = json!([
        { "name": "bad", "language": "text", "kind": "invalid",
          "files": [{ "path": "a.txt", "body": "x" }], "expect": [{ "line": 1 }] },
        { "name": "good", "language": "text", "kind": "valid",
          "files": [{ "path": "a.txt", "body": "x" }] },
    ]);
    spec["provenance"] = json!({});
    let text = format!(
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/review\nspec: {spec}\n"
    );
    let catalog = Catalog::from_local(
        BTreeMap::from([("review.yaml".to_owned(), text)]),
        &lighthouse_test_support::cel(),
    )
    .unwrap();
    let plugin = Checks::new(&lighthouse_test_support::composition(), "local", &catalog).unwrap();
    let options = json!({ "kinds": [kind], "envy": limits(kind, overrides) });
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
    candidates(world, "type", overrides)
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
fn feature_envy_skips_short_functions() {
    let (mut w, summary) = envious(&["items", "total", "extra"]);
    w.summarize(&summary, 2, &[]);
    assert!(envy(&w, json!({})).is_empty());
    assert_eq!(envy(&w, json!({ "minStatements": 2 })).len(), 1);
}

#[test]
fn feature_envy_skips_entry_points_and_wiring() {
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

const MODULE: &str = "design/module-placement";
const FILE: &str = "design/file-placement";
const METHOD: &str = "design/prefer-method";

/// The functions `design/module-placement` puts to a model.
fn moves(w: &World) -> Vec<String> {
    selected(w, MODULE, "symbol")
}

/// The functions `design/file-placement` puts to a model.
fn misfiled(w: &World) -> Vec<String> {
    selected(w, FILE, "symbol")
}

/// Gives `f` parameters of the types in `modules` (`module::Name`).
fn takes(w: &mut World, f: &Symbol, types: &[&str]) {
    let summary = w.summaries.iter_mut().find(|s| s.symbol == f.id).unwrap();
    summary.signature.params = types
        .iter()
        .map(|id| TypeRef {
            text: (*id).to_owned(),
            symbol: Some((*id).to_owned()),
            exported: Some(true),
        })
        .collect();
}

/// A private `clone` of module `interp` that takes a `types.Value` and calls
/// the functions `names` of module `types`; `load` of `interp` calls it.
fn cloner(names: &[&str]) -> (World, Symbol) {
    let mut w = World::default();
    w.symbol("types", "Value", SymbolKind::Type, "types/value.ucm");
    let clone = w.func("interp", "clone", "interp/a.ucm");
    w.summarize(&clone, 4, &[]);
    w.private(&clone);
    takes(&mut w, &clone, &["types::Value"]);
    for name in names {
        let used = w.func("types", name, "types/value.ucm");
        w.edge(EdgeKind::Calls, &clone, &used);
    }
    let load = w.func("interp", "load", "interp/b.ucm");
    w.edge(EdgeKind::Calls, &load, &clone);
    w.import("interp", "types");
    (w, clone)
}

#[test]
fn placement_moves_a_function_that_uses_only_one_other_module() {
    let (w, _) = cloner(&["array", "map", "struct"]);
    assert_eq!(moves(&w), ["clone"]);
    // One finding for the symbol: every finer decision is silent.
    assert!(misfiled(&w).is_empty());
    assert!(w.check(METHOD, json!({})).is_empty());
    assert!(w.check("design/top-down-order", json!({})).is_empty());
}

#[test]
fn placement_keeps_a_helper_that_uses_its_own_module() {
    let (mut w, clone) = cloner(&["array", "map", "struct"]);
    let state = w.symbol("interp", "state", SymbolKind::Var, "interp/a.ucm");
    w.edge(EdgeKind::References, &clone, &state);
    assert!(moves(&w).is_empty());
}

#[test]
fn placement_keeps_a_function_the_target_cannot_import() {
    // `log` imports `types`: a `clone` in `types` would import `log` back.
    let (mut w, clone) = cloner(&["array", "map", "struct"]);
    let write = w.func("log", "write", "log/a.ucm");
    w.edge(EdgeKind::Calls, &clone, &write);
    w.import("log", "types");
    assert!(moves(&w).is_empty(), "types would import log");
}

#[test]
fn placement_needs_a_clear_winner_among_several_modules() {
    // Three symbols of `types` and one of `log`: `types` wins and is admissible.
    let (mut w, clone) = cloner(&["array", "map", "struct"]);
    let write = w.func("log", "write", "log/a.ucm");
    w.edge(EdgeKind::Calls, &clone, &write);
    assert_eq!(moves(&w), ["clone"]);

    // Two and two: moving to either would be moved back, so no move.
    let (mut w, clone) = cloner(&["array", "map"]);
    for name in ["write", "flush"] {
        let used = w.func("log", name, "log/a.ucm");
        w.edge(EdgeKind::Calls, &clone, &used);
    }
    assert!(moves(&w).is_empty());
}

#[test]
fn placement_leaves_the_order_of_a_function_that_is_a_method_in_all_but_name() {
    // Called only by methods of `Store`, which it takes: `design/prefer-method`
    // reports it, a model is not asked to move it, and its order waits.
    let mut w = World::default();
    let store = w.symbol("m", "Store", SymbolKind::Type, "m/a.ucm");
    let total = w.member(&store, "Total", SymbolKind::Method, "m/a.ucm");
    let sum = w.func("m", "sum", "m/a.ucm");
    w.summarize(&sum, 4, &[]);
    w.private(&sum);
    w.edge(EdgeKind::Calls, &total, &sum);
    takes(&mut w, &sum, &["m::Store"]);
    assert_eq!(w.names(&w.check(METHOD, json!({}))), ["sum"]);
    assert!(moves(&w).is_empty());
}

#[test]
fn placement_keeps_a_function_whose_signature_names_its_own_type() {
    // It takes `Store` and a `types.Value`: its signature is not the
    // vocabulary of `types` alone, so it stays in `interp`.
    let (mut w, clone) = cloner(&["array", "map", "struct"]);
    let store = w.symbol("interp", "Store", SymbolKind::Type, "interp/a.ucm");
    let total = w.member(&store, "Total", SymbolKind::Method, "interp/a.ucm");
    w.edge(EdgeKind::Calls, &total, &clone);
    takes(&mut w, &clone, &["interp::Store", "types::Value"]);
    assert!(moves(&w).is_empty());
}

#[test]
fn placement_selects_a_misfiled_helper_by_file_only() {
    let mut w = World::default();
    let total = w.func("m", "total", "m/a.ucm");
    w.summarize(&total, 4, &[]);
    w.private(&total);
    for name in ["first", "second", "third"] {
        let used = w.func("m", name, "m/b.ucm");
        w.edge(EdgeKind::Calls, &total, &used);
    }
    let run = w.func("m", "run", "m/c.ucm");
    w.edge(EdgeKind::Calls, &run, &total);
    assert_eq!(misfiled(&w), ["total"]);
    assert!(moves(&w).is_empty());
    // A wrong file is no level: the order of the function is still judged.
    assert!(w.check("design/top-down-order", json!({})).is_empty());

    // Every caller sits beside it: it lives with what it serves.
    w.symbols.iter_mut().find(|s| s.id == run.id).unwrap().file = "m/a.ucm".into();
    assert!(misfiled(&w).is_empty());
}
