//! The ordering and naming rules over synthetic projects: the cases the
//! catalog examples cannot reach, such as the shape of a project the real
//! providers do not produce.

mod support;

use lighthouse_model::{EdgeKind, SymbolKind};
use serde_json::json;
use support::World;

const GROUPS: &str = "design/declaration-groups";
const CLOSE: &str = "design/related-symbols-close";
const CALLERS: &str = "design/callers-before-callees";
const HELPERS: &str = "design/private-helper-callers";
const BANNERS: &str = "design/section-banners";
const QUALIFIERS: &str = "design/no-redundant-qualifiers";

#[test]
fn groups_flag_the_fewest_declarations_that_break_the_order() {
    let mut w = World::default();
    let helper = w.func("m", "helper", "m/a.ucm");
    w.private(&helper);
    w.symbol("m", "A", SymbolKind::Type, "m/a.ucm");
    w.symbol("m", "B", SymbolKind::Type, "m/a.ucm");
    w.symbol("m", "Limit", SymbolKind::Const, "m/a.ucm");
    let found = w.check(GROUPS, json!({}));
    assert_eq!(
        w.names(&found),
        ["helper"],
        "one misplaced helper, not three"
    );
}

#[test]
fn groups_accept_a_file_in_order_and_ignore_unlisted_groups() {
    let mut w = World::default();
    w.symbol("m", "A", SymbolKind::Type, "m/a.ucm");
    w.symbol("m", "Limit", SymbolKind::Const, "m/a.ucm");
    w.func("m", "Run", "m/a.ucm");
    let helper = w.func("m", "helper", "m/a.ucm");
    w.private(&helper);
    w.symbol("m", "Late", SymbolKind::Type, "m/a.ucm");
    assert_eq!(w.names(&w.check(GROUPS, json!({}))), ["Late"]);
    let only = json!({ "groups": ["public-function", "private-function"] });
    assert!(
        w.check(GROUPS, only).is_empty(),
        "types and consts are not ordered"
    );
}

#[test]
fn groups_put_init_before_functions_and_constructors_after_them() {
    let mut w = World::default();
    w.func("m", "Run", "m/a.ucm");
    w.func("m", "NewThing", "m/a.ucm");
    w.func("m", "Open", "m/a.ucm");
    let found = w.check(GROUPS, json!({}));
    assert_eq!(
        w.names(&found),
        ["Open"],
        "Open is a function and follows no constructor"
    );
}

#[test]
fn groups_skip_test_files_members_of_interfaces_and_nested_functions() {
    let mut w = World::default();
    let helper = w.func("m", "helper", "m/a_test.ucm");
    w.private(&helper);
    w.symbol("m", "A", SymbolKind::Type, "m/a_test.ucm");
    let iface = w.symbol("n", "Reader", SymbolKind::Interface, "n/a.ucm");
    w.member(&iface, "Read", SymbolKind::Method, "n/a.ucm");
    let outer = w.func("n", "Outer", "n/a.ucm");
    let inner = w.member(&outer, "inner", SymbolKind::Function, "n/a.ucm");
    w.private(&inner);
    assert!(w.check(GROUPS, json!({})).is_empty());
}

#[test]
fn constructors_come_first_within_a_type_when_asked() {
    let mut w = World::default();
    let ty = w.symbol("m", "A", SymbolKind::Type, "m/a.ucm");
    w.member(&ty, "get", SymbolKind::Method, "m/a.ucm");
    w.member(&ty, "new", SymbolKind::Method, "m/a.ucm");
    let options = json!({
        "groups": ["type"],
        "constructor_prefixes": ["new"],
        "constructors_first": true,
    });
    let found = w.check(GROUPS, options);
    assert_eq!(w.names(&found), ["new"]);
    let relaxed = json!({ "groups": ["type"], "constructor_prefixes": ["new"] });
    assert!(w.check(GROUPS, relaxed).is_empty());
}

#[test]
fn close_flags_a_method_separated_from_its_owner_by_a_foreign_one() {
    let mut w = World::default();
    let a = w.symbol("m", "A", SymbolKind::Type, "m/a.ucm");
    let b = w.symbol("m", "B", SymbolKind::Type, "m/a.ucm");
    w.member(&a, "One", SymbolKind::Method, "m/a.ucm");
    w.member(&b, "One", SymbolKind::Method, "m/a.ucm");
    w.member(&a, "Two", SymbolKind::Method, "m/a.ucm");
    let found = w.check(CLOSE, json!({}));
    assert_eq!(w.names(&found), ["Two"]);
}

#[test]
fn close_flags_a_free_function_between_methods() {
    let mut w = World::default();
    let a = w.symbol("m", "A", SymbolKind::Type, "m/a.ucm");
    w.member(&a, "One", SymbolKind::Method, "m/a.ucm");
    w.func("m", "Free", "m/a.ucm");
    w.member(&a, "Two", SymbolKind::Method, "m/a.ucm");
    assert_eq!(w.names(&w.check(CLOSE, json!({}))), ["Two"]);
}

#[test]
fn close_tolerates_other_visibility_groups_unless_told_not_to() {
    let mut w = World::default();
    let a = w.symbol("m", "A", SymbolKind::Type, "m/a.ucm");
    let b = w.symbol("m", "B", SymbolKind::Type, "m/a.ucm");
    w.member(&a, "One", SymbolKind::Method, "m/a.ucm");
    let hidden = w.member(&b, "hidden", SymbolKind::Method, "m/a.ucm");
    w.private(&hidden);
    w.member(&a, "Two", SymbolKind::Method, "m/a.ucm");
    assert!(
        w.check(CLOSE, json!({})).is_empty(),
        "a private method of B is in B's group"
    );
    let strict = w.check(CLOSE, json!({ "visibility_groups": false }));
    assert_eq!(w.names(&strict), ["Two"]);
}

#[test]
fn close_accepts_adjacent_members_of_an_owner() {
    let mut w = World::default();
    let a = w.symbol("m", "A", SymbolKind::Type, "m/a.ucm");
    let b = w.symbol("m", "B", SymbolKind::Type, "m/a.ucm");
    w.member(&a, "One", SymbolKind::Method, "m/a.ucm");
    w.member(&a, "Two", SymbolKind::Method, "m/a.ucm");
    w.member(&b, "One", SymbolKind::Method, "m/a.ucm");
    assert!(w.check(CLOSE, json!({})).is_empty());
}

/// `Run` calls `helper`; returns both and the world with `helper` private.
fn call_pair(helper_first: bool) -> (World, lighthouse_model::Symbol, lighthouse_model::Symbol) {
    let mut w = World::default();
    let (run, helper) = if helper_first {
        let helper = w.func("m", "helper", "m/a.ucm");
        (w.func("m", "Run", "m/a.ucm"), helper)
    } else {
        let run = w.func("m", "Run", "m/a.ucm");
        (run, w.func("m", "helper", "m/a.ucm"))
    };
    w.private(&helper);
    w.edge(EdgeKind::Calls, &run, &helper);
    (w, run, helper)
}

#[test]
fn callers_come_before_their_private_callees() {
    let (w, ..) = call_pair(true);
    assert_eq!(w.names(&w.check(CALLERS, json!({}))), ["helper"]);
    let (w, ..) = call_pair(false);
    assert!(w.check(CALLERS, json!({})).is_empty());
}

#[test]
fn callers_leave_shared_other_file_value_and_recursive_helpers_alone() {
    let (mut w, _, helper) = call_pair(true);
    let other = w.func("m", "Other", "m/b.ucm");
    w.edge(EdgeKind::Calls, &other, &helper);
    assert!(
        w.check(CALLERS, json!({})).is_empty(),
        "a caller in another file"
    );

    let (mut w, run, helper) = call_pair(true);
    w.edge(EdgeKind::References, &run, &helper);
    assert!(w.check(CALLERS, json!({})).is_empty(), "used as a value");

    let (mut w, run, helper) = call_pair(true);
    w.edge(EdgeKind::Calls, &helper, &run);
    assert!(w.check(CALLERS, json!({})).is_empty(), "mutual recursion");

    let (mut w, _, helper) = call_pair(true);
    w.edge(EdgeKind::Calls, &helper, &helper);
    assert_eq!(
        w.names(&w.check(CALLERS, json!({}))),
        ["helper"],
        "self recursion alone is fine"
    );
}

#[test]
fn callers_ignore_test_callers_and_can_ask_for_the_last_caller() {
    let (mut w, _, helper) = call_pair(false);
    let test = w.func("m", "TestHelper", "m/a_test.ucm");
    w.edge(EdgeKind::Calls, &test, &helper);
    assert!(w.check(CALLERS, json!({})).is_empty());

    let mut w = World::default();
    let first = w.func("m", "First", "m/a.ucm");
    let leaf = w.func("m", "leaf", "m/a.ucm");
    let second = w.func("m", "Second", "m/a.ucm");
    w.private(&leaf);
    w.edge(EdgeKind::Calls, &first, &leaf);
    w.edge(EdgeKind::Calls, &second, &leaf);
    assert!(w.check(CALLERS, json!({})).is_empty());
    let strict = w.check(CALLERS, json!({ "shared_after_last_caller": true }));
    assert_eq!(w.names(&strict), ["leaf"]);
}

#[test]
fn helpers_with_one_production_caller_are_reported() {
    let (w, ..) = call_pair(false);
    assert_eq!(w.names(&w.check(HELPERS, json!({}))), ["helper"]);
}

#[test]
fn helpers_with_two_callers_or_a_test_caller_pass() {
    let (mut w, _, helper) = call_pair(false);
    let other = w.func("m", "Other", "m/a.ucm");
    w.edge(EdgeKind::Calls, &other, &helper);
    assert!(w.check(HELPERS, json!({})).is_empty(), "two callers");

    let (mut w, _, helper) = call_pair(false);
    let test = w.func("m", "TestHelper", "m/a_test.ucm");
    w.edge(EdgeKind::Calls, &test, &helper);
    assert_eq!(
        w.names(&w.check(HELPERS, json!({}))),
        ["helper"],
        "tests do not count"
    );
}

#[test]
fn helpers_leave_wrappers_to_their_own_rule_and_skip_values_and_recursion() {
    let (mut w, _, helper) = call_pair(false);
    let target = w.func("m", "target", "m/a.ucm");
    w.private(&target);
    w.edge(EdgeKind::Calls, &helper, &target);
    w.forward(&helper, &target);
    let found = w.check(HELPERS, json!({}));
    assert_eq!(
        w.names(&found),
        ["target"],
        "the wrapper is not reported twice"
    );

    let (mut w, run, helper) = call_pair(false);
    w.edge(EdgeKind::References, &run, &helper);
    assert!(w.check(HELPERS, json!({})).is_empty());
}

#[test]
fn banners_are_comments_made_only_of_banner_lines() {
    let mut w = World::default();
    w.comment("m/a.ucm", 1, "// ---- Helpers ----");
    w.comment("m/a.ucm", 2, "// ==========");
    w.comment("m/a.ucm", 3, "/* *** Types *** */");
    w.comment("m/a.ucm", 4, "// MARK: - Public");
    w.comment("m/a.ucm", 5, "// -- two is not enough");
    w.comment("m/a.ucm", 6, "// Format:\n// ---\n// end");
    w.comment("m/a.ucm", 7, "/// ---");
    w.comment("m/a.ucm", 8, "// ---- a\n// ---- b");
    let found = w.check(BANNERS, json!({}));
    let lines: Vec<u32> = found.iter().map(|(_, l)| *l).collect();
    assert_eq!(lines, [1, 2, 3, 4, 8]);
}

#[test]
fn banners_are_tunable_and_skip_generated_files() {
    let mut w = World::default();
    w.comment("m/a.ucm", 1, "// ---- x");
    w.comment("m/a.ucm", 2, "// -- x");
    w.comment("m/a.ucm", 3, "// REGION x");
    let options = json!({ "min_run": 2, "labels": ["REGION"] });
    let lines: Vec<u32> = w.check(BANNERS, options).iter().map(|(_, l)| *l).collect();
    assert_eq!(lines, [1, 2, 3]);
    w.files.insert("m/a.ucm".to_owned(), true);
    assert!(w.check(BANNERS, json!({})).is_empty());
}

#[test]
fn qualifiers_flag_names_that_carry_their_module_name() {
    let mut w = World::default();
    w.module("provider", Some("provider"), None);
    w.symbol(
        "provider",
        "ProviderOptions",
        SymbolKind::Type,
        "provider/a.ucm",
    );
    w.symbol(
        "provider",
        "OptionsProvider",
        SymbolKind::Type,
        "provider/a.ucm",
    );
    w.symbol("provider", "Provider", SymbolKind::Type, "provider/a.ucm");
    w.symbol("provider", "Providence", SymbolKind::Type, "provider/a.ucm");
    w.symbol("provider", "Options", SymbolKind::Type, "provider/a.ucm");
    let hidden = w.symbol(
        "provider",
        "providerState",
        SymbolKind::Type,
        "provider/a.ucm",
    );
    w.private(&hidden);
    let found = w.check(QUALIFIERS, json!({}));
    assert_eq!(w.names(&found), ["ProviderOptions", "OptionsProvider"]);
}

#[test]
fn qualifiers_split_acronyms_and_match_the_last_path_component() {
    let mut w = World::default();
    w.module("sdk", None, None);
    w.symbol("sdk", "SDKFrame", SymbolKind::Type, "sdk/a.ucm");
    w.symbol("sdk", "SDK", SymbolKind::Type, "sdk/a.ucm");
    w.symbol("sdk", "SDKs", SymbolKind::Type, "sdk/a.ucm");
    w.module("lib/store", None, None);
    w.symbol(
        "lib/store",
        "StoreHandle",
        SymbolKind::Type,
        "lib/store/a.ucm",
    );
    let found = w.check(QUALIFIERS, json!({}));
    assert_eq!(w.names(&found), ["StoreHandle", "SDKFrame"]);
}

#[test]
fn qualifiers_skip_members_tests_and_the_listed_kinds() {
    let mut w = World::default();
    w.module("store", None, None);
    let ty = w.symbol("store", "Store", SymbolKind::Type, "store/a.ucm");
    w.member(&ty, "StoreGet", SymbolKind::Method, "store/a.ucm");
    w.symbol("store", "StoreTest", SymbolKind::Type, "store/a_test.ucm");
    w.symbol("store", "StoreLimit", SymbolKind::Const, "store/a.ucm");
    assert_eq!(w.names(&w.check(QUALIFIERS, json!({}))), ["StoreLimit"]);
    let only_types = w.check(QUALIFIERS, json!({ "kinds": ["type"] }));
    assert!(only_types.is_empty());
}
