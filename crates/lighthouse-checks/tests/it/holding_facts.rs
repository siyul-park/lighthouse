//! The `holders` and `hold_cycle` facts of a type: who holds it, and the
//! strongly connected component of the holding graph that contains it.

use crate::func_facts::flagged;
use crate::support::World;
use lighthouse_model::ucm::{SymbolKind, TypeRef};
use serde_json::json;

const FILE: &str = "m/a.ucm";

/// A type `name` of module `m` in [`FILE`].
fn ty(w: &mut World, name: &str) -> lighthouse_model::ucm::Symbol {
    w.symbol("m", name, SymbolKind::Type, FILE)
}

/// A field of `holder` whose type is `held`.
fn hold(
    w: &mut World,
    holder: &lighthouse_model::ucm::Symbol,
    held: &lighthouse_model::ucm::Symbol,
) {
    let mut field = w.member(holder, &format!("f{}", held.name), SymbolKind::Field, FILE);
    field.type_ref = Some(TypeRef {
        text: format!("*{}", held.name),
        symbol: Some(held.id.as_str().trim_end_matches("#type").to_owned()),
        exported: None,
    });
    *w.symbols.last_mut().unwrap() = field;
}

/// The names of the types for which `condition` holds.
fn types_where(w: &World, condition: &str) -> Vec<String> {
    let lines = flagged(
        w,
        "symbol",
        &format!("symbol.kind == 'type' && {condition}"),
        &json!({}),
    );
    lines
        .into_iter()
        .map(|(_, line)| w.symbols[line as usize - 1].name.clone())
        .collect()
}

#[test]
fn types_that_hold_each_other_in_a_circle_share_a_cycle() {
    let mut w = World::default();
    let (a, b, c) = (ty(&mut w, "A"), ty(&mut w, "B"), ty(&mut w, "C"));
    hold(&mut w, &a, &b);
    hold(&mut w, &b, &c);
    hold(&mut w, &c, &a);

    let found = types_where(&w, "size(symbol.hold_cycle) == 3");

    assert_eq!(found, ["A", "B", "C"]);
}

#[test]
fn a_type_that_holds_itself_is_a_cycle_of_one() {
    let mut w = World::default();
    let a = ty(&mut w, "A");
    hold(&mut w, &a, &a);

    let found = types_where(&w, "symbol.hold_cycle == [symbol.id]");

    assert_eq!(found, ["A"]);
}

#[test]
fn a_type_on_no_cycle_has_none_and_lists_its_holder() {
    let mut w = World::default();
    let (a, b) = (ty(&mut w, "A"), ty(&mut w, "B"));
    hold(&mut w, &a, &b);

    let cycle = types_where(&w, "size(symbol.hold_cycle) > 0");
    let held = types_where(&w, "size(symbol.holders) == 1");

    assert!(cycle.is_empty());
    assert_eq!(held, ["B"]);
}

#[test]
fn a_field_whose_resolved_symbol_is_empty_is_read_by_its_text() {
    let mut w = World::default();
    let (a, b) = (ty(&mut w, "A"), ty(&mut w, "B"));
    hold(&mut w, &a, &b);
    hold(&mut w, &b, &a);
    for field in w.symbols.iter_mut().filter(|s| s.kind == SymbolKind::Field) {
        let type_ref = field.type_ref.as_mut().unwrap();
        type_ref.text = type_ref.text.replace('*', "*/m.");
        type_ref.symbol = Some(String::new());
    }

    let found = types_where(&w, "size(symbol.hold_cycle) == 2");

    assert_eq!(found, ["A", "B"]);
}

#[test]
fn a_file_of_many_types_is_read_once() {
    const TYPES: usize = 500;
    const FIELDS: usize = 4;
    let mut w = World::default();
    let types: Vec<_> = (0..TYPES).map(|i| ty(&mut w, &format!("T{i}"))).collect();
    for (i, holder) in types.iter().enumerate() {
        for step in 1..=FIELDS {
            hold(&mut w, holder, &types[(i + step) % TYPES]);
        }
    }

    let start = std::time::Instant::now();
    let found = types_where(
        &w,
        "size(symbol.hold_cycle) == 500 && size(symbol.holders) == 4",
    );

    assert_eq!(found.len(), TYPES);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}
