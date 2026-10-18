//! The brief an agent is given, and the examples it shows.

use lighthouse_engine::{
    FixPlan,
    brief::{MAX_EXAMPLES, examples_for, safe_fixes},
};
use lighthouse_spec::{Catalog, Example};
use serde_json::json;

fn example(name: &str, kind: &str, fixed: bool) -> Example {
    let fixed = if fixed {
        json!([{ "path": "a.go", "body": "y" }])
    } else {
        json!([])
    };
    serde_json::from_value(json!({
        "name": name,
        "language": "go",
        "kind": kind,
        "files": [{ "path": "a.go", "body": "x" }],
        "fixed": fixed,
    }))
    .unwrap()
}

#[test]
fn examples_for_keeps_the_invalid_examples_that_show_a_fix_and_at_most_the_limit() {
    let all = [
        example("valid", "valid", false),
        example("unfixed", "invalid", false),
        example("one", "invalid", true),
        example("two", "invalid", true),
        example("three", "invalid", true),
    ];

    let shown = examples_for(&all);

    let names: Vec<&str> = shown.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["one", "two"]);
    assert_eq!(shown.len(), MAX_EXAMPLES);
}

#[test]
fn safe_fixes_are_the_decisions_with_a_safe_fix_in_the_language() {
    let plan = FixPlan::from_catalog(Catalog::bundled());

    let rust = safe_fixes(&plan, Some("rust"));
    let none = safe_fixes(&plan, Some("cobol"));
    let any = safe_fixes(&plan, None);

    assert!(rust.contains(&"design/declaration-groups"), "{rust:?}");
    assert!(none.is_empty(), "{none:?}");
    assert!(any.len() >= rust.len());
}

#[test]
fn fix_plan_bindings_name_the_rules_with_a_fix_in_the_order_of_their_ids() {
    let plan = FixPlan::from_catalog(Catalog::bundled());

    let rules: Vec<&str> = plan.bindings().map(|(rule, _)| rule).collect();

    let mut sorted = rules.clone();
    sorted.sort_unstable();
    assert_eq!(rules, sorted);
    assert!(rules.contains(&"design/declaration-groups"));
}
