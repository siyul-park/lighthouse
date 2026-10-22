//! The `func.unit` of a function (its own measures and those of its
//! single-use helpers) and the `parent` of a module (what it shares with the
//! module that holds it), read by cel checks over synthetic projects.

use lighthouse_model::ucm::{EdgeKind, Flow, FlowKind, Symbol};
use serde_json::json;

use crate::{func_facts::flagged, support::World};

/// The names of the functions of `world` for which `condition` holds.
fn functions(world: &World, condition: &str) -> Vec<String> {
    let found: Vec<_> = flagged(world, "function", condition, &json!({}))
        .into_iter()
        .collect();
    world.names(&found)
}

/// A private function of module `m` with `statements` statements and as many
/// `if`s as `ifs` says.
fn helper(w: &mut World, name: &str, statements: u32, ifs: usize) -> Symbol {
    let symbol = w.func("m", name, "m/a.ucm");
    w.private(&symbol);
    w.summarize(&symbol, statements, &vec![Flow::new(FlowKind::If, 0); ifs]);
    symbol
}

fn calls(w: &mut World, from: &Symbol, to: &Symbol) {
    w.edge(EdgeKind::Calls, from, to);
}

#[test]
fn a_unit_adds_the_helpers_of_the_helpers() {
    let mut w = World::default();
    let f = helper(&mut w, "f", 3, 2);
    let h1 = helper(&mut w, "h1", 2, 1);
    let h2 = helper(&mut w, "h2", 4, 0);
    calls(&mut w, &f, &h1);
    calls(&mut w, &h1, &h2);
    let sums = "func.unit.lines == 3 && func.unit.statements == 9 && func.unit.cyclomatic == 4 && func.unit.cognitive == 3";
    assert_eq!(functions(&w, &format!("func.name == 'f' && {sums}")), ["f"]);
    assert_eq!(
        functions(&w, "func.unit.helpers.map(h, h.name) == ['h1', 'h2']"),
        ["f"]
    );
    assert_eq!(functions(&w, "func.unit.helpers.size() == 1"), ["h1"]);
    assert_eq!(
        functions(
            &w,
            "func.unit.caller != null && func.unit.caller.name == 'f'"
        ),
        ["h1"]
    );
    assert_eq!(
        functions(&w, "func.unit.caller != null"),
        ["h1", "h2"],
        "f is no helper"
    );
}

#[test]
fn a_unit_without_helpers_measures_the_function_alone() {
    let mut w = World::default();
    let f = helper(&mut w, "f", 3, 2);
    let metrics = "func.metrics.statements == func.unit.statements && func.metrics.cyclomatic == func.unit.cyclomatic && func.metrics.cognitive == func.unit.cognitive && func.metrics.lines == func.unit.lines";
    assert_eq!(
        functions(&w, &format!("{metrics} && func.unit.helpers == []")),
        [f.name.as_str()]
    );
}

#[test]
fn a_function_called_as_a_method_is_computed_too() {
    let mut w = World::default();
    let f = helper(&mut w, "f", 3, 2);
    assert_eq!(
        functions(&w, "func.metrics.statements == 3"),
        [f.name.as_str()]
    );
}

#[test]
fn a_helper_with_two_callers_is_not_folded() {
    let mut w = World::default();
    let f = helper(&mut w, "f", 1, 0);
    let g = helper(&mut w, "g", 1, 0);
    let h = helper(&mut w, "h", 5, 0);
    calls(&mut w, &f, &h);
    calls(&mut w, &g, &h);
    assert!(functions(&w, "func.unit.helpers.size() > 0").is_empty());
    assert!(functions(&w, "func.unit.caller != null").is_empty());
}

#[test]
fn a_helper_used_as_a_value_is_not_folded() {
    let mut w = World::default();
    let f = helper(&mut w, "f", 1, 0);
    let h = helper(&mut w, "h", 5, 0);
    calls(&mut w, &f, &h);
    w.edge(EdgeKind::References, &f, &h);
    assert!(functions(&w, "func.unit.helpers.size() > 0").is_empty());
}

#[test]
fn a_public_function_is_not_a_helper() {
    let mut w = World::default();
    let f = helper(&mut w, "f", 1, 0);
    let h = w.func("m", "H", "m/a.ucm");
    calls(&mut w, &f, &h);
    assert!(functions(&w, "func.unit.helpers.size() > 0").is_empty());
}

#[test]
fn a_recursive_helper_is_not_folded() {
    let mut w = World::default();
    let f = helper(&mut w, "f", 1, 0);
    let h = helper(&mut w, "h", 5, 0);
    calls(&mut w, &f, &h);
    calls(&mut w, &h, &h);
    assert!(functions(&w, "func.unit.helpers.size() > 0").is_empty());
}

#[test]
fn helpers_that_call_each_other_are_folded_once() {
    let mut w = World::default();
    let f = helper(&mut w, "f", 1, 0);
    let h1 = helper(&mut w, "h1", 2, 0);
    let h2 = helper(&mut w, "h2", 3, 0);
    calls(&mut w, &f, &h1);
    calls(&mut w, &h1, &h2);
    calls(&mut w, &h2, &h1);
    assert!(
        functions(&w, "func.name == 'f' && func.unit.helpers.size() > 0").is_empty(),
        "h1 has two callers"
    );

    let mut pair = World::default();
    let a = helper(&mut pair, "a", 1, 0);
    let b = helper(&mut pair, "b", 2, 0);
    calls(&mut pair, &a, &b);
    calls(&mut pair, &b, &a);
    assert_eq!(
        functions(
            &pair,
            "func.unit.statements == 3 && func.unit.helpers.size() == 1"
        ),
        ["a", "b"],
        "the walk ends at the root"
    );
}

#[test]
fn a_helper_of_tests_only_has_no_production_caller() {
    let mut w = World::default();
    let t = w.func("m", "check", "m/a_test.ucm");
    let h = helper(&mut w, "h", 5, 0);
    calls(&mut w, &t, &h);
    assert!(functions(&w, "func.unit.caller != null").is_empty());

    let f = helper(&mut w, "f", 1, 0);
    calls(&mut w, &f, &h);
    assert_eq!(
        functions(&w, "func.unit.helpers.size() == 1"),
        ["f"],
        "a test calling it too does not count"
    );
}

/// A function of module `module` that calls `callee`.
fn caller(w: &mut World, module: &str, file: &str, name: &str, callee: &Symbol) -> Symbol {
    let symbol = w.func(module, name, file);
    w.edge(EdgeKind::Calls, &symbol, callee);
    symbol
}

/// Modules: `crate` with a private `secret`; `crate/tied` that calls it and is
/// called by the parent; `crate/open` that only uses the parent's public
/// `api`; `crate/shared`, which `other` uses as well.
fn family() -> (World, Symbol) {
    let mut w = World::default();
    for path in ["crate", "crate/tied", "crate/open", "crate/shared", "other"] {
        w.module(path, None, None);
    }
    let secret = w.func("crate", "secret", "crate/lib.ucm");
    w.private(&secret);
    let api = w.func("crate", "api", "crate/lib.ucm");
    let tied = caller(&mut w, "crate/tied", "crate/tied/a.ucm", "tie", &secret);
    let open = caller(&mut w, "crate/open", "crate/open/a.ucm", "open", &api);
    let shared = caller(
        &mut w,
        "crate/shared",
        "crate/shared/a.ucm",
        "share",
        &secret,
    );
    for child in [&tied, &open, &shared] {
        caller(
            &mut w,
            "crate",
            "crate/lib.ucm",
            &format!("use_{}", child.name),
            child,
        );
    }
    caller(&mut w, "other", "other/a.ucm", "elsewhere", &shared);
    (w, secret)
}

/// The first files of the modules of `world` for which `condition` holds.
fn modules(world: &World, condition: &str) -> Vec<String> {
    flagged(world, "module", condition, &json!({}))
        .into_iter()
        .map(|(file, _)| file)
        .collect()
}

#[test]
fn a_child_that_reaches_private_parts_of_its_parent_and_is_called_back_is_tied() {
    let (w, secret) = family();
    let tied = format!(
        "module.parent != null && module.parent.path == 'crate' && module.parent.sole_user && module.parent.called && module.parent.reaches.map(r, r.id) == ['{}']",
        secret.id.as_str()
    );
    assert_eq!(modules(&w, &tied), ["crate/tied/a.ucm"]);
}

#[test]
fn a_child_that_uses_only_the_public_parts_of_its_parent_reaches_nothing() {
    let (w, _) = family();
    assert_eq!(
        modules(
            &w,
            "module.parent != null && module.parent.sole_user && module.parent.called && module.parent.reaches == []"
        ),
        ["crate/open/a.ucm"]
    );
}

#[test]
fn a_child_with_a_second_user_is_not_the_sole_one() {
    let (w, _) = family();
    assert_eq!(
        modules(
            &w,
            "module.parent != null && !module.parent.sole_user && module.parent.reaches.size() == 1"
        ),
        ["crate/shared/a.ucm"]
    );
}

#[test]
fn a_module_whose_parent_is_no_module_of_the_project_has_no_parent() {
    let (w, _) = family();
    assert_eq!(
        modules(&w, "module.parent == null"),
        ["crate/lib.ucm", "other/a.ucm"]
    );
}
