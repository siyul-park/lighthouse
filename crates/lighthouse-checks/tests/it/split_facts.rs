//! The `split` fact of a method declared away from its type's file: how far
//! the file it lives in is a file of that type's own concern.

use lighthouse_checks::probe;
use lighthouse_model::{
    Reach,
    ucm::{Applicability, EdgeKind, Project, Symbol, SymbolKind},
};
use lighthouse_plugin::{Context, Memo, Notices, Workspace};
use serde_json::{Value, json};

use crate::support::World;

const HOME: &str = "m/store.ucm";
const FAR: &str = "m/total.ucm";
const OTHER: &str = "m/other.ucm";

fn fact(project: &Project, name: &str) -> Value {
    let symbol: Symbol = project
        .symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap()
        .clone();
    let ws = Workspace::new(".");
    let facts = Default::default();
    let memo = Memo::default();
    let notices = Notices::default();
    let ctx = Context {
        workspace: &ws,
        project,
        file: None,
        facts: &facts,
        trusted: false,
        memo: &memo,
        annotations: &lighthouse_plugin::Annotations::default(),
        notices: &notices,
        applies: Applicability::default(),
        models: &lighthouse_plugin::NoModels,
        queries: &lighthouse_plugin::NoQueries,
    };
    probe::symbol_fact(&lighthouse_test_support::cel(), &ctx, &symbol, "split").unwrap()
}

/// `Store` in its home file with two methods `a` and `b` in `FAR`, `a`
/// calling `b`.
fn concern() -> (World, Symbol, Symbol) {
    let mut w = World::default();
    let store = w.symbol("m", "Store", SymbolKind::Type, HOME);
    let a = w.member(&store, "a", SymbolKind::Method, FAR);
    let b = w.member(&store, "b", SymbolKind::Method, FAR);
    w.edge(EdgeKind::Calls, &a, &b);
    let user = w.func("m", "use_a", OTHER);
    w.edge(EdgeKind::Calls, &user, &a);
    (w, a, b)
}

#[test]
fn a_file_of_the_types_methods_is_dedicated_to_it() {
    let (w, ..) = concern();
    assert_eq!(
        fact(&w.project(), "a"),
        json!({ "share": 1.0, "home": HOME, "home_methods": 0, "methods": 2 })
    );
}

#[test]
fn the_methods_of_the_type_at_home_and_in_all_are_counted() {
    let (mut w, ..) = concern();
    let store = w
        .symbols
        .iter()
        .find(|s| s.name == "Store")
        .unwrap()
        .clone();
    w.member(&store, "near", SymbolKind::Method, HOME);
    let value = fact(&w.project(), "a");
    assert_eq!(
        (value["home_methods"].clone(), value["methods"].clone()),
        (json!(1), json!(3))
    );
}

#[test]
fn a_method_in_its_types_file_has_no_split() {
    let mut w = World::default();
    let store = w.symbol("m", "Store", SymbolKind::Type, HOME);
    w.member(&store, "near", SymbolKind::Method, HOME);
    assert_eq!(fact(&w.project(), "near"), json!({}));
}

#[test]
fn a_function_of_the_file_that_no_method_reaches_lowers_the_share() {
    let (mut w, ..) = concern();
    w.func("m", "stray", FAR);
    assert_eq!(fact(&w.project(), "a")["share"], json!(2.0 / 3.0));
}

#[test]
fn a_private_helper_that_only_the_methods_reach_counts_for_the_methods() {
    let (mut w, a, _) = concern();
    let helper = w.func("m", "helper", FAR);
    w.private(&helper);
    w.edge(EdgeKind::Calls, &a, &helper);
    w.func("m", "stray", FAR);
    assert_eq!(fact(&w.project(), "a")["share"], json!(0.75));
}

#[test]
fn a_private_helper_another_function_of_the_file_uses_does_not_count() {
    let (mut w, a, _) = concern();
    let helper = w.func("m", "helper", FAR);
    w.private(&helper);
    let stray = w.func("m", "stray", FAR);
    w.edge(EdgeKind::Calls, &a, &helper);
    w.edge(EdgeKind::Calls, &stray, &helper);
    assert_eq!(fact(&w.project(), "a")["share"], json!(0.5));
}

#[test]
fn a_helper_used_outside_its_file_is_no_local_helper() {
    let (mut w, a, _) = concern();
    let helper = w.func("m", "helper", FAR);
    w.private(&helper);
    w.edge(EdgeKind::Calls, &a, &helper);
    let user = w.func("m", "use_helper", OTHER);
    w.edge(EdgeKind::Calls, &user, &helper);
    let value = fact(&w.project(), "a");
    assert_eq!(value["share"], json!(2.0 / 3.0));
}

#[test]
fn the_fact_reads_the_whole_project_so_it_reaches_globally() {
    let reach = lighthouse_checks::caching::FACTS
        .iter()
        .find(|(word, _)| *word == "split")
        .map(|(_, reach)| *reach);
    assert_eq!(reach, Some(Reach::Global));
    // A method that moves into the file in another file changes the fact of
    // one that stays.
    let (mut w, ..) = concern();
    let before = fact(&w.project(), "a");
    let store = w
        .symbols
        .iter()
        .find(|s| s.name == "Store")
        .unwrap()
        .clone();
    w.member(&store, "c", SymbolKind::Method, FAR);
    assert_ne!(fact(&w.project(), "a"), before);
}
