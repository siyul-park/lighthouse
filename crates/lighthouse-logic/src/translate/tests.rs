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

fn holds(ucq: &Ucq, subject: Value, options: Value) -> bool {
    ucq.holds(&subject, &json!(null), &options)
}

#[test]
fn not_exists_of_true_is_an_empty_container() {
    let ucq = query("symbol", json!([]), "!symbol.callers.exists(c, true)").unwrap();

    assert_eq!(ucq.cqs[0].empties.len(), 1);
    assert!(holds(&ucq, json!({"callers": []}), json!({})));
    assert!(!holds(&ucq, json!({"callers": [1]}), json!({})));
}

#[test]
fn membership_in_an_option_list_or_an_option_map() {
    let list = query("symbol", json!([]), "symbol.k in options.kinds").unwrap();
    let map = query("symbol", json!([]), "symbol.k in options.keys").unwrap();

    assert!(holds(
        &list,
        json!({"k": "a"}),
        json!({"kinds": ["a", "b"]})
    ));
    assert!(!holds(
        &list,
        json!({"k": "c"}),
        json!({"kinds": ["a", "b"]})
    ));
    assert!(holds(&map, json!({"k": "a"}), json!({"keys": {"a": 1}})));
    assert!(!holds(&map, json!({"k": "b"}), json!({"keys": {"a": 1}})));
}

#[test]
fn a_size_is_that_of_a_list_a_map_or_a_text() {
    let none = query("symbol", json!([]), "size(symbol.x) == 0").unwrap();
    let some = query("symbol", json!([]), "size(symbol.x) > 0").unwrap();

    for (value, empty) in [
        (json!(""), true),
        (json!("ab"), false),
        (json!({}), true),
        (json!({"k": 1}), false),
        (json!([]), true),
        (json!([1]), false),
    ] {
        assert_eq!(
            holds(&none, json!({"x": value}), json!({})),
            empty,
            "{value}"
        );
        assert_eq!(
            holds(&some, json!({"x": value}), json!({})),
            !empty,
            "{value}"
        );
    }
}

#[test]
fn members_of_a_map_are_its_keys() {
    let ucq = query("symbol", json!([]), "symbol.m.exists(k, k == 'a')").unwrap();

    assert!(holds(&ucq, json!({"m": {"a": 1}}), json!({})));
    assert!(!holds(&ucq, json!({"m": {"b": 1}}), json!({})));
}

#[test]
fn numbers_compare_as_the_cel_compares_them() {
    let over = query("symbol", json!([]), "symbol.n > 2").unwrap();
    let after = query("symbol", json!([]), "symbol.s > 'a'").unwrap();

    assert!(holds(&over, json!({"n": 2.5}), json!({})));
    assert!(!holds(&over, json!({"n": 2.0}), json!({})));
    assert!(holds(&after, json!({"s": "b"}), json!({})));
    assert!(!holds(&after, json!({"s": 3}), json!({})));
}

#[test]
fn a_chain_of_variables_that_doubles_is_refused_past_a_cap() {
    let chain = |depth: usize| {
        let mut variables = vec![json!({"name": "v0", "expression": "symbol.a == 1"})];
        for at in 1..=depth {
            let name = format!("v{at}");
            let expression = format!("v{} && v{}", at - 1, at - 1);
            variables.push(json!({"name": name, "expression": expression}));
        }
        query("symbol", Value::Array(variables), &format!("v{depth}"))
    };

    assert!(chain(4).is_ok());
    assert!(chain(10).unwrap_err().0.contains("too many conditions"));
    assert!(chain(16).unwrap_err().0.contains("expands too far"));
}

#[test]
fn a_condition_with_too_many_cases_is_refused() {
    let case = |at: usize| format!("(symbol.a{at} == 1 || symbol.a{at} == 2)");
    let all: Vec<String> = (0..13).map(case).collect();

    let found = rejected(&all.join(" && "));

    assert!(found.contains("too many cases"), "{found}");
}

#[test]
fn a_bound_value_that_is_never_read_is_refused() {
    let variables = json!([{"name": "f", "params": ["x"], "expression": "true"}]);

    let unused_bind = rejected("cel.bind(k, symbol.x.y, true)");
    let unused_param = query("symbol", variables, "f(symbol.a)").unwrap_err().0;

    assert!(unused_bind.contains("never read"), "{unused_bind}");
    assert!(unused_param.contains("never read"), "{unused_param}");
}

#[test]
fn arithmetic_is_only_for_a_comparison_with_a_number() {
    assert!(rejected("(symbol.callers + 1).exists(c, c.t)").contains("outside a comparison"));
    assert!(rejected("(symbol.n + 1).y == 1").contains("outside a comparison"));
    assert!(rejected("symbol.n + 1 == 'a'").contains("outside a comparison"));
    assert!(rejected("symbol.n + 9223372036854775807 + 1 > 0").contains("overflows"));
}

#[test]
fn what_is_refused_is_named_by_its_construct() {
    assert!(rejected("symbol.a == (symbol.b ? 1 : 2)").contains("ternary"));
    assert!(rejected("symbol.name == 'a' + symbol.b").contains("concatenation"));
    assert!(rejected("symbol.m['k'] == 1").contains("index"));
    assert!(rejected("declaration_group > 1").contains("undefined name or pack variable"));
    assert!(rejected("symbol.k in symbol.kinds").contains("membership in a field"));
}
