//! The `func.steps_level` and `func.abstraction_mix` of a function: its steps
//! with what each calls, and how they spread over the heights of the call
//! graph, read by cel checks over synthetic projects.

use lighthouse_model::{
    Position, Span,
    ucm::{EdgeKind, Step, StepCall, StepKind, Symbol, SymbolKind},
};
use serde_json::json;

use crate::{
    func_facts::flagged,
    support::{World, at},
};

/// The names of the functions of `world` for which `condition` holds.
fn functions(world: &World, condition: &str) -> Vec<String> {
    let found: Vec<_> = flagged(world, "function", condition, &json!({}))
        .into_iter()
        .collect();
    world.names(&found)
}

fn span(line: u32) -> Span {
    Span {
        start: Position { line, col: 1 },
        end: Position {
            line: line + 1,
            col: 1,
        },
    }
}

fn step(line: u32, kind: StepKind, calls: &[(&str, u32)]) -> Step {
    Step {
        span: span(line),
        kind,
        operators: 0,
        indexing: 0,
        literals: 0,
        calls: calls
            .iter()
            .map(|(name, at_line)| StepCall {
                name: (*name).to_owned(),
                site: at(*at_line),
            })
            .collect(),
        nested: 0,
        guard: false,
    }
}

/// A function with `steps`.
fn function(w: &mut World, name: &str, steps: Vec<Step>) -> Symbol {
    let symbol = w.func("m", name, "m/a.ucm");
    w.steps(&symbol, steps);
    symbol
}

/// `from` calls `to` at `line`.
fn call(w: &mut World, from: &Symbol, to: &Symbol, line: u32) {
    w.edge_at(EdgeKind::Calls, from, to, line);
}

#[test]
fn a_function_is_one_higher_than_the_tallest_function_it_calls() {
    let mut w = World::default();
    let h = function(&mut w, "h", vec![step(1, StepKind::Call, &[])]);
    let g = function(&mut w, "g", vec![step(1, StepKind::Call, &[("h", 1)])]);
    let f = function(&mut w, "f", vec![step(1, StepKind::Call, &[("g", 1)])]);
    call(&mut w, &g, &h, 1);
    call(&mut w, &f, &g, 1);
    for (name, height) in [("h", 0), ("g", 1), ("f", 2)] {
        let condition = format!("func.name == '{name}' && func.abstraction_mix.height == {height}");
        assert_eq!(functions(&w, &condition), [name]);
    }
}

#[test]
fn functions_that_call_each_other_share_a_height() {
    let mut w = World::default();
    let a = function(&mut w, "a", vec![step(1, StepKind::Call, &[("b", 1)])]);
    let b = function(
        &mut w,
        "b",
        vec![step(1, StepKind::Call, &[("a", 1), ("c", 2)])],
    );
    let c = function(&mut w, "c", vec![step(1, StepKind::Call, &[])]);
    call(&mut w, &a, &b, 1);
    call(&mut w, &b, &a, 1);
    call(&mut w, &b, &c, 2);
    assert_eq!(
        functions(&w, "func.abstraction_mix.height == 1"),
        ["a", "b"]
    );
}

#[test]
fn an_abstract_method_has_the_height_of_its_tallest_implementation() {
    let mut w = World::default();
    let h = function(&mut w, "h", vec![step(1, StepKind::Call, &[])]);
    let g = function(&mut w, "g", vec![step(1, StepKind::Call, &[("h", 1)])]);
    call(&mut w, &g, &h, 1);
    let iface = w.symbol("m", "I", SymbolKind::Interface, "m/a.ucm");
    let abstract_method = w.member(&iface, "M", SymbolKind::Method, "m/a.ucm");
    w.summaries.retain(|s| s.symbol != abstract_method.id);
    let ty = w.symbol("m", "T", SymbolKind::Type, "m/a.ucm");
    let implementation = w.member(&ty, "M", SymbolKind::Method, "m/a.ucm");
    w.implement(&implementation.id);
    w.steps(&implementation, vec![step(1, StepKind::Call, &[("g", 1)])]);
    call(&mut w, &implementation, &g, 1);
    w.edge(EdgeKind::Implements, &ty, &iface);
    let f = function(&mut w, "f", vec![step(1, StepKind::Call, &[("M", 1)])]);
    call(&mut w, &f, &abstract_method, 1);
    assert_eq!(
        functions(&w, "func.abstraction_mix.height == 3"),
        ["f"],
        "I.M stands for T.M, which is 2 high"
    );
}

#[test]
fn a_step_has_the_height_of_the_tallest_project_function_it_calls() {
    let mut w = World::default();
    let h = function(&mut w, "h", vec![step(1, StepKind::Call, &[])]);
    let g = function(&mut w, "g", vec![step(1, StepKind::Call, &[("h", 1)])]);
    call(&mut w, &g, &h, 1);
    let mut mixed = step(
        1,
        StepKind::Assign,
        &[("g", 1), ("h", 2), ("strings.Join", 3)],
    );
    mixed.operators = 2;
    let f = function(&mut w, "f", vec![mixed]);
    call(&mut w, &f, &g, 1);
    call(&mut w, &f, &h, 2);
    let found = functions(
        &w,
        "func.name == 'f' && func.steps_level[0].named_level == 1 && func.steps_level[0].calls_project == 2 && func.steps_level[0].calls_external == 1 && func.steps_level[0].primitive == 2 && !func.steps_level[0].neutral",
    );
    assert_eq!(found, ["f"]);
}

#[test]
fn a_step_that_calls_no_project_function_has_no_named_level() {
    let mut w = World::default();
    let mut counting = step(1, StepKind::Assign, &[("len", 1)]);
    counting.indexing = 1;
    let f = function(&mut w, "f", vec![counting]);
    let found = functions(
        &w,
        "func.steps_level[0].named_level == null && func.steps_level[0].calls[0].name == 'len' && !func.steps_level[0].calls[0].project",
    );
    assert_eq!(found, [f.name.as_str()]);
}

#[test]
fn a_guard_and_a_step_that_calls_and_counts_nothing_are_neutral() {
    let mut w = World::default();
    let mut guard = step(1, StepKind::If, &[]);
    guard.guard = true;
    let bare = step(3, StepKind::Return, &[]);
    let declared = step(4, StepKind::Declare, &[]);
    let empty_assign = step(5, StepKind::Assign, &[]);
    let mut value = step(6, StepKind::Return, &[]);
    value.literals = 1;
    let calling = step(7, StepKind::Call, &[("log", 7)]);
    let f = function(
        &mut w,
        "f",
        vec![guard, bare, declared, empty_assign, value, calling],
    );
    let found = functions(
        &w,
        "func.steps_level.map(s, s.neutral) == [true, true, true, true, false, false]",
    );
    assert_eq!(found, [f.name.as_str()]);
}

/// `I` with an abstract `M`, implemented by `T` (and by whatever `shape`
/// makes of it), and `f`, which calls `I.M`; the height of `f`.
fn height_through_interface(w: &mut World, shape: impl FnOnce(&mut World, &Symbol)) -> bool {
    let h = function(w, "h", vec![step(1, StepKind::Call, &[])]);
    let g = function(w, "g", vec![step(1, StepKind::Call, &[("h", 1)])]);
    call(w, &g, &h, 1);
    let iface = w.symbol("m", "I", SymbolKind::Interface, "m/a.ucm");
    let abstract_method = w.member(&iface, "M", SymbolKind::Method, "m/a.ucm");
    w.summaries.retain(|s| s.symbol != abstract_method.id);
    let ty = w.symbol("m", "T", SymbolKind::Type, "m/a.ucm");
    w.edge(EdgeKind::Implements, &ty, &iface);
    shape(w, &ty);
    let f = function(w, "f", vec![step(1, StepKind::Call, &[("M", 1)])]);
    call(w, &f, &abstract_method, 1);
    // g is 1 high: an implementation that calls g is 2, and f is 3.
    !functions(w, "func.name == 'f' && func.abstraction_mix.height == 3").is_empty()
}

#[test]
fn a_method_a_type_embeds_is_one_of_its_implementations() {
    let mut w = World::default();
    let tall = height_through_interface(&mut w, |w, ty| {
        let inner = w.symbol("m", "U", SymbolKind::Type, "m/a.ucm");
        let m = w.member(&inner, "M", SymbolKind::Method, "m/a.ucm");
        w.implement(&m.id);
        let g = w.symbols.iter().find(|s| s.name == "g").unwrap().clone();
        w.steps(&m, vec![step(1, StepKind::Call, &[("g", 1)])]);
        call(w, &m, &g, 1);
        let mut field = w.member(ty, "U", SymbolKind::Field, "m/a.ucm");
        field.embedded = true;
        field.type_ref = Some(lighthouse_model::ucm::TypeRef {
            text: "U".to_owned(),
            symbol: Some(inner.id.as_str().trim_end_matches("#type").to_owned()),
            exported: None,
        });
        *w.symbols.last_mut().unwrap() = field;
    });
    assert!(tall, "T.M is U.M, which calls g");
}

#[test]
fn an_inherent_method_of_the_same_name_is_not_an_implementation() {
    let mut w = World::default();
    let tall = height_through_interface(&mut w, |w, ty| {
        let g = w.symbols.iter().find(|s| s.name == "g").unwrap().clone();
        let m = w.member(ty, "M", SymbolKind::Method, "m/a.ucm");
        w.steps(&m, vec![step(1, StepKind::Call, &[("g", 1)])]);
        call(w, &m, &g, 1);
    });
    assert!(!tall, "T.M implements nothing; it only shares the name");
}

#[test]
fn the_mix_counts_the_steps_at_each_offset_from_the_step_down_level() {
    let mut w = World::default();
    let h = function(&mut w, "h", vec![step(1, StepKind::Call, &[])]);
    let g = function(&mut w, "g", vec![step(1, StepKind::Call, &[("h", 1)])]);
    call(&mut w, &g, &h, 1);
    let mut raw = step(5, StepKind::Assign, &[]);
    raw.operators = 3;
    let mut guard = step(7, StepKind::If, &[]);
    guard.guard = true;
    let steps = vec![
        step(1, StepKind::Call, &[("g", 1)]),
        step(2, StepKind::Call, &[("h", 2)]),
        step(3, StepKind::Call, &[("h", 3)]),
        raw,
        guard,
    ];
    let f = function(&mut w, "f", steps);
    call(&mut w, &f, &g, 1);
    call(&mut w, &f, &h, 2);
    call(&mut w, &f, &h, 3);
    let mix = "func.abstraction_mix.height == 2 && func.abstraction_mix.step_down == 1 && func.abstraction_mix.steps == 4 && func.abstraction_mix.unnamed == 1 && func.abstraction_mix.offsets == {'0': 1, '-1': 2}";
    assert_eq!(
        functions(&w, &format!("func.name == 'f' && {mix}")),
        [f.name.as_str()]
    );
}

#[test]
fn a_function_without_steps_has_no_facts() {
    let mut w = World::default();
    w.func("m", "f", "m/a.ucm");
    assert_eq!(
        functions(
            &w,
            "func.steps_level == null && func.abstraction_mix == null"
        ),
        ["f"]
    );
}

#[test]
fn a_field_named_as_its_type_that_embeds_nothing_brings_no_methods() {
    let mut w = World::default();
    let tall = height_through_interface(&mut w, |w, ty| {
        let inner = w.symbol("m", "U", SymbolKind::Type, "m/a.ucm");
        let m = w.member(&inner, "M", SymbolKind::Method, "m/a.ucm");
        w.implement(&m.id);
        let g = w.symbols.iter().find(|s| s.name == "g").unwrap().clone();
        w.steps(&m, vec![step(1, StepKind::Call, &[("g", 1)])]);
        call(w, &m, &g, 1);
        let mut field = w.member(ty, "U", SymbolKind::Field, "m/a.ucm");
        field.type_ref = Some(lighthouse_model::ucm::TypeRef {
            text: "U".to_owned(),
            symbol: Some(inner.id.as_str().trim_end_matches("#type").to_owned()),
            exported: None,
        });
        *w.symbols.last_mut().unwrap() = field;
    });
    assert!(!tall, "U is a field of T, not part of it");
}

#[test]
fn a_trait_impl_method_implements_its_own_trait_only() {
    let mut w = World::default();
    let tall = height_through_interface(&mut w, |w, ty| {
        let g = w.symbols.iter().find(|s| s.name == "g").unwrap().clone();
        let other = w.symbol("m", "Other", SymbolKind::Interface, "m/a.ucm");
        let other_method = w.member(&other, "M", SymbolKind::Method, "m/a.ucm");
        let m = w.member(ty, "M", SymbolKind::Method, "m/a.ucm");
        w.implement(&m.id);
        w.steps(&m, vec![step(1, StepKind::Call, &[("g", 1)])]);
        call(w, &m, &g, 1);
        let summary = w.summaries.iter_mut().find(|s| s.symbol == m.id).unwrap();
        summary.implements = Some(other_method.id.as_str().to_owned());
    });
    assert!(!tall, "T.M implements Other.M, not I.M");
}
