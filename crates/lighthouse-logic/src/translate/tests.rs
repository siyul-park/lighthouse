use lighthouse_spec::{CelCheck, Env};
use serde_json::{Value, json};

use super::{Reject, translate};
use crate::Ucq;

fn check(variables: Value, condition: &str) -> CelCheck {
    serde_json::from_value(json!({
        "variables": variables,
        "where": condition,
        "message": "m",
    }))
    .unwrap()
}

fn query(subject: &str, variables: Value, condition: &str) -> Result<Ucq, Reject> {
    let env = Env::standard().unwrap();
    translate(&env, &check(variables, condition), subject)
}

fn rejected(condition: &str) -> String {
    query("symbol", json!([]), condition).unwrap_err().0
}

#[test]
fn a_field_compared_with_an_option_is_one_conjunction() {
    let ucq = query("file", json!([]), "file.lines > options.max").unwrap();

    assert_eq!(ucq.cqs.len(), 1);
    assert!(ucq.holds(&json!({"lines": 11}), &json!(null), &json!({"max": 10})));
    assert!(!ucq.holds(&json!({"lines": 10}), &json!(null), &json!({"max": 10})));
}

#[test]
fn an_offset_moves_into_the_constant() {
    let ucq = query("symbol", json!([]), "symbol.n + 1 > 5").unwrap();

    assert!(ucq.holds(&json!({"n": 5}), &json!(null), &json!({})));
    assert!(!ucq.holds(&json!({"n": 4}), &json!(null), &json!({})));
    assert!(ucq.to_string().contains("S.n > 4"), "{ucq}");
}

#[test]
fn variables_are_inlined_and_a_ternary_is_two_cases() {
    let variables = json!([
        {"name": "private", "expression": "symbol.visibility == 'private'"},
        {"name": "limit", "expression": "private ? 3 : 9"},
    ]);
    let found = query("symbol", variables, "private && symbol.n in [1, 2]").unwrap();

    assert!(found.holds(
        &json!({"visibility": "private", "n": 2}),
        &json!(null),
        &json!({})
    ));
    assert!(!found.holds(
        &json!({"visibility": "public", "n": 2}),
        &json!(null),
        &json!({})
    ));
    assert!(!found.holds(
        &json!({"visibility": "private", "n": 3}),
        &json!(null),
        &json!({})
    ));
}

#[test]
fn a_ternary_condition_is_the_union_of_its_cases() {
    let ucq = query(
        "symbol",
        json!([]),
        "symbol.a ? symbol.n > 1 : symbol.n < 0",
    )
    .unwrap();

    assert_eq!(ucq.cqs.len(), 2);
    assert!(ucq.holds(&json!({"a": true, "n": 2}), &json!(null), &json!({})));
    assert!(ucq.holds(&json!({"a": false, "n": -1}), &json!(null), &json!({})));
    assert!(!ucq.holds(&json!({"a": false, "n": 2}), &json!(null), &json!({})));
}

#[test]
fn a_variable_with_parameters_is_applied_by_name() {
    let variables = json!([
        {"name": "big", "params": ["x"], "expression": "x.n > options.max"},
    ]);
    let ucq = query("symbol", variables, "symbol.callers.exists(c, big(c))").unwrap();

    let symbol = json!({"callers": [{"n": 1}, {"n": 12}]});
    assert!(ucq.holds(&symbol, &json!(null), &json!({"max": 10})));
    assert!(!ucq.holds(
        &json!({"callers": [{"n": 1}]}),
        &json!(null),
        &json!({"max": 10})
    ));
}

#[test]
fn exists_filter_and_size_read_as_some_member_or_none() {
    let some = query("symbol", json!([]), "symbol.callers.exists(c, c.test)").unwrap();
    let filtered = query(
        "symbol",
        json!([]),
        "size(symbol.callers.filter(c, c.test)) > 0",
    )
    .unwrap();
    let none = query("symbol", json!([]), "size(symbol.callers) == 0").unwrap();
    let with = json!({"callers": [{"test": false}, {"test": true}]});
    let without = json!({"callers": [{"test": false}]});
    let empty = json!({"callers": []});

    for ucq in [&some, &filtered] {
        assert!(ucq.holds(&with, &json!(null), &json!({})));
        assert!(!ucq.holds(&without, &json!(null), &json!({})));
    }
    assert!(none.holds(&empty, &json!(null), &json!({})));
    assert!(!none.holds(&with, &json!(null), &json!({})));
}

#[test]
fn bind_names_a_value_and_a_list_literal_has_a_known_size() {
    let ucq = query(
        "symbol",
        json!([{"name": "m", "expression": "symbol.a ? ['x'] : []"}]),
        "cel.bind(k, symbol.n, k > 2) && size(m) > 0",
    )
    .unwrap();

    assert!(ucq.holds(&json!({"a": true, "n": 3}), &json!(null), &json!({})));
    assert!(!ucq.holds(&json!({"a": false, "n": 3}), &json!(null), &json!({})));
    assert!(!ucq.holds(&json!({"a": true, "n": 2}), &json!(null), &json!({})));
}

#[test]
fn string_tests_and_negation_flip_the_operator() {
    let ucq = query(
        "symbol",
        json!([]),
        "!symbol.name.startsWith('x') && symbol.k != 'b'",
    )
    .unwrap();

    assert!(ucq.holds(&json!({"name": "abc", "k": "a"}), &json!(null), &json!({})));
    assert!(!ucq.holds(&json!({"name": "xbc", "k": "a"}), &json!(null), &json!({})));
}

#[test]
fn what_leaves_the_fragment_is_named() {
    assert!(rejected("symbol.name.lowerAscii() == 'a'").contains("lowerAscii"));
    assert!(rejected("symbol.a > symbol.b").contains("two fields"));
    assert!(rejected("symbol.callers.all(c, c.test)").contains("all"));
    assert!(rejected("!symbol.callers.exists(c, c.test)").contains("negation"));
    assert!(rejected("size(symbol.callers) > 1").contains("count"));
    assert!(rejected("size(symbol.callers) == 1").contains("count"));
    assert!(rejected("symbol.callers.map(c, c.n).exists(n, n > 1)").contains("comprehension"));
    assert!(rejected("unknown_variable > 1").contains("unknown_variable"));
}

#[test]
fn a_missing_field_is_no_violation() {
    let ucq = query("symbol", json!([]), "symbol.func.metrics.fan_in > 1").unwrap();

    assert!(!ucq.holds(&json!({"func": null}), &json!(null), &json!({})));
}
