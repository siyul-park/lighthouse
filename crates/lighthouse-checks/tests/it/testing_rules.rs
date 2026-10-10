//! The test-contract rules over synthetic projects.

use crate::support::{Subject, World};
use lighthouse_checks::Pack;
use lighthouse_model::{EdgeKind, Resolution, Symbol, SymbolKind, Visibility};
use lighthouse_spec::Catalog;
use serde_json::{Value, json};

const EXTERNAL: &str = "testing/external-test-package";
const OWNER: &str = "testing/owner-test";
const SINGLE: &str = "testing/single-owner-test";

fn check(w: &World, rule: &str, options: Value) -> Vec<(String, u32)> {
    let testing = Pack::of("testing", Catalog::bundled());
    w.check_in(
        &Subject {
            plugin: &testing,
            id: "testing",
        },
        rule,
        options,
    )
}

/// A package `m` with public `Get` and `Put`, an external test module `m[test]`
/// and the given tests, each calling the named symbol.
fn package(tests: &[(&str, Option<&str>)]) -> (World, Vec<Symbol>) {
    let mut w = World::default();
    w.module("m", Some("m"), None);
    w.module("m[test]", Some("m_test"), Some("m"));
    let get = w.func("m", "Get", "m/a.ucm");
    let put = w.func("m", "Put", "m/a.ucm");
    let mut made = vec![get.clone(), put.clone()];
    for (name, calls) in tests {
        let test = w.symbol("m[test]", name, SymbolKind::Test, "m/a_test.ucm");
        let target = match *calls {
            Some("Get") => Some(&get),
            Some("Put") => Some(&put),
            _ => None,
        };
        if let Some(target) = target {
            w.edge(EdgeKind::Calls, &test, target);
        }
        w.test_case(&test, &target.into_iter().collect::<Vec<_>>());
        made.push(test);
    }
    (w, made)
}

#[test]
fn owner_test_flags_a_public_symbol_no_test_touches() {
    let (w, _) = package(&[("TestGet", Some("Get"))]);
    assert_eq!(w.names(&check(&w, OWNER, json!({}))), ["Put"]);
}

#[test]
fn owner_test_accepts_a_named_owner_or_a_test_that_uses_the_symbol() {
    let (w, _) = package(&[("TestGet", None), ("TestPutFlow", Some("Put"))]);
    assert!(check(&w, OWNER, json!({})).is_empty());
}

#[test]
fn owner_test_judges_only_modules_with_tests_and_public_symbols() {
    let mut w = World::default();
    w.module("m", None, None);
    w.func("m", "Get", "m/a.ucm");
    assert!(check(&w, OWNER, json!({})).is_empty(), "no tests at all");

    let (mut w, made) = package(&[("TestGet", Some("Get"))]);
    let put = made[1].clone();
    w.private(&put);
    assert!(check(&w, OWNER, json!({})).is_empty(), "private symbol");
    w.symbols
        .iter_mut()
        .find(|s| s.id == put.id)
        .unwrap()
        .visibility = Visibility::Internal;
    assert!(
        check(&w, OWNER, json!({})).is_empty(),
        "internal by default"
    );
    assert_eq!(
        w.names(&check(&w, OWNER, json!({ "include_internal": true }))),
        ["Put"]
    );
}

#[test]
fn owner_test_counts_a_member_use_for_its_type_and_exempts_hooks() {
    let (mut w, _) = package(&[("TestGet", None)]);
    let ty = w.symbol("m", "Store", SymbolKind::Type, "m/a.ucm");
    let method = w.member(&ty, "Load", SymbolKind::Method, "m/a.ucm");
    w.member(&ty, "String", SymbolKind::Method, "m/a.ucm");
    let found = w.names(&check(&w, OWNER, json!({})));
    assert_eq!(found, ["Put", "Store", "Load"]);
    let test = w
        .symbols
        .iter()
        .find(|s| s.name == "TestGet")
        .unwrap()
        .clone();
    w.edge(EdgeKind::Calls, &test, &method);
    let found = w.names(&check(&w, OWNER, json!({})));
    assert_eq!(
        found,
        ["Put"],
        "Store is tested through Load; String is a hook"
    );
}

#[test]
fn single_owner_flags_variants_of_one_symbol() {
    let (w, _) = package(&[
        ("TestGet", None),
        ("TestGet_Missing", None),
        ("TestPut", None),
    ]);
    assert_eq!(w.names(&check(&w, SINGLE, json!({}))), ["Get"]);
    let exact_only = check(&w, SINGLE, json!({ "variant_tests": false }));
    assert!(exact_only.is_empty());
}

#[test]
fn single_owner_judges_internal_symbols_only_when_asked() {
    let (mut w, made) = package(&[("TestGet", None), ("TestGet_Missing", None)]);
    let get = made[0].clone();
    w.symbols
        .iter_mut()
        .find(|s| s.id == get.id)
        .unwrap()
        .visibility = Visibility::Internal;
    assert!(
        check(&w, SINGLE, json!({})).is_empty(),
        "internal by default"
    );
    assert_eq!(
        w.names(&check(&w, SINGLE, json!({ "include_internal": true }))),
        ["Get"]
    );
}

#[test]
fn single_owner_lets_a_longer_name_belong_to_the_method() {
    let mut w = World::default();
    w.module("m", None, None);
    w.module("m[test]", None, Some("m"));
    let ty = w.symbol("m", "Store", SymbolKind::Type, "m/a.ucm");
    w.member(&ty, "Get", SymbolKind::Method, "m/a.ucm");
    for name in ["TestStore", "TestStore_Get", "TestStore_Get_Missing"] {
        let test = w.symbol("m[test]", name, SymbolKind::Test, "m/a_test.ucm");
        w.test_case(&test, &[]);
    }
    let found = w.names(&check(&w, SINGLE, json!({})));
    assert_eq!(found, ["Get"], "Store has one owner; Get has two");
}

#[test]
fn single_owner_reads_snake_case_names() {
    let mut w = World::default();
    w.module("c", None, None);
    w.module("c[test:a]", None, Some("c"));
    w.module("c[test:b]", None, Some("c"));
    w.func("c", "parse_header", "c/a.ucm");
    for module in ["c[test:a]", "c[test:b]"] {
        let test = w.symbol(module, "parse_header", SymbolKind::Test, "c/tests/a.ucm");
        w.test_case(&test, &[]);
    }
    let options = json!({ "test_prefix": "", "snake_case": true, "variant_tests": false });
    assert_eq!(w.names(&check(&w, SINGLE, options)), ["parse_header"]);
}

/// A package with a private function reached by a test file of the package.
fn internal(external: bool) -> World {
    let mut w = World::default();
    w.module("m", None, None);
    let test_module = if external { "m[test]" } else { "m" };
    if external {
        w.module("m[test]", None, Some("m"));
    }
    let hidden = w.func("m", "hidden", "m/a.ucm");
    w.private(&hidden);
    let test = w.symbol(test_module, "TestHidden", SymbolKind::Test, "m/a_test.ucm");
    w.edge(EdgeKind::Calls, &test, &hidden);
    w
}

#[test]
fn external_flags_an_internal_test_file_that_reaches_private_symbols() {
    let w = internal(false);
    assert_eq!(
        check(&w, EXTERNAL, json!({})),
        [("m/a_test.ucm".to_owned(), 2)]
    );
}

#[test]
fn external_accepts_external_packages_and_public_use() {
    assert!(check(&internal(true), EXTERNAL, json!({})).is_empty());

    let mut w = internal(false);
    let hidden = w.symbols[0].clone();
    w.symbols[0].visibility = Visibility::Public;
    let _ = hidden;
    assert!(
        check(&w, EXTERNAL, json!({})).is_empty(),
        "only public symbols used"
    );
}

#[test]
fn owner_test_exempts_data_types_and_trait_impl_methods() {
    let (mut w, _) = package(&[("TestGet", Some("Get")), ("TestPut", Some("Put"))]);
    let data = w.symbol("m", "Data", SymbolKind::Type, "m/a.ucm");
    w.member(&data, "field", SymbolKind::Field, "m/a.ucm");
    let object = w.symbol("m", "Object", SymbolKind::Type, "m/a.ucm");
    let mut via_trait = w.member(&object, "fmt", SymbolKind::Method, "m/a.ucm");
    via_trait.id =
        lighthouse_model::SymbolId::new("m", &["Object", "Display"], "fmt", SymbolKind::Method);
    *w.symbols.last_mut().unwrap() = via_trait;
    let found = w.names(&check(&w, OWNER, json!({})));
    assert_eq!(
        found,
        ["Object"],
        "Data has no methods; fmt is a trait impl"
    );
    let all = w.names(&check(&w, OWNER, json!({ "include_data_types": true })));
    assert_eq!(all, ["Data", "Object"]);
}

/// A crate `c` with a private submodule `c::sub` that declares `parse`, and an
/// integration-test module of the crate root with the named tests.
fn crate_with_submodule(tests: &[&str]) -> (World, Symbol) {
    let mut w = World::default();
    w.module("c", None, None);
    w.module("c/sub", None, None);
    w.module("c[test:api]", None, Some("c"));
    let parse = w.func("c/sub", "parse", "c/sub.ucm");
    for name in tests {
        let test = w.symbol("c[test:api]", name, SymbolKind::Test, "c/tests/api.ucm");
        w.test_case(&test, &[]);
    }
    (w, parse)
}

fn rust_naming() -> Value {
    json!({ "test_prefix": "", "snake_case": true, "variant_tests": false, "ancestor_tests": true })
}

#[test]
fn owner_test_credits_integration_tests_of_the_crate_root_to_its_submodules() {
    let (w, _) = crate_with_submodule(&["parse"]);
    assert!(check(&w, OWNER, rust_naming()).is_empty());
    let (w, _) = crate_with_submodule(&["other"]);
    assert_eq!(w.names(&check(&w, OWNER, rust_naming())), ["parse"]);
}

#[test]
fn owner_test_accepts_a_heuristic_reference_from_test_code() {
    let (mut w, parse) = crate_with_submodule(&["other"]);
    let test = w
        .symbols
        .iter()
        .find(|s| s.name == "other")
        .unwrap()
        .clone();
    w.edge(EdgeKind::References, &test, &parse);
    w.edges.last_mut().unwrap().resolution = Resolution::Heuristic;
    assert!(check(&w, OWNER, rust_naming()).is_empty());
}

#[test]
fn owner_test_credits_ancestor_tests_only_when_asked() {
    let (w, _) = crate_with_submodule(&["parse"]);
    let mut options = rust_naming();
    options["ancestor_tests"] = json!(false);
    assert_eq!(w.names(&check(&w, OWNER, options)), ["parse"]);
}

#[test]
fn single_owner_keeps_names_within_one_module() {
    let (w, _) = crate_with_submodule(&["parse"]);
    let mut options = rust_naming();
    options.as_object_mut().unwrap().remove("ancestor_tests");
    assert!(check(&w, SINGLE, options).is_empty());
}
