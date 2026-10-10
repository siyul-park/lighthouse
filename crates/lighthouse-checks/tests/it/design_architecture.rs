//! The decisions about where modules and types sit: layers, type names and
//! module size, over synthetic projects.

use crate::support::World;
use lighthouse_model::SymbolKind;
use serde_json::{Value, json};

const LAYERS: &str = "design/layers";
const TYPE_NAMES: &str = "design/unique-type-names";
const TINY: &str = "design/tiny-modules";

/// A project whose modules each declare one function, named like the last
/// segment of the module path, and import each other as `imports` say.
fn modules(paths: &[&str], imports: &[(&str, &str)]) -> World {
    let mut w = World::default();
    for path in paths {
        let name = path.rsplit('/').next().unwrap();
        w.module(path, None, None);
        w.symbol(path, name, SymbolKind::Function, &format!("{path}/a.ucm"));
    }
    for (from, to) in imports {
        w.import(from, to);
    }
    w
}

/// The importers the layers decision reports, by the name of their module.
fn importers(paths: &[&str], imports: &[(&str, &str)], options: Value) -> Vec<String> {
    let w = modules(paths, imports);
    w.names(&w.check(LAYERS, options))
}

#[test]
fn layers_report_nothing_until_layers_or_forbidden_are_configured() {
    let found = importers(&["domain", "infra"], &[("infra", "domain")], json!({}));
    assert!(found.is_empty());
}

#[test]
fn layers_report_an_import_that_points_up() {
    let layers = json!({ "layers": [["cli"], ["app/domain"], ["app/infra"]] });
    let paths = ["cli", "app/domain", "app/infra"];
    let imports = [
        ("cli", "app/domain"),
        ("app/domain", "app/infra"),
        ("app/infra", "app/domain"),
        ("app/infra", "cli"),
    ];
    assert_eq!(
        importers(&paths, &imports, layers),
        ["infra".to_owned(), "infra".to_owned()]
    );
}

#[test]
fn layers_skip_modules_no_layer_names_and_modules_outside_the_project() {
    let layers = json!({ "layers": [["top"], ["bottom"]] });
    let imports = [("bottom", "other"), ("other", "top"), ("bottom", "fmt")];
    assert!(importers(&["top", "bottom", "other"], &imports, layers).is_empty());
}

#[test]
fn layers_globs_take_stars_for_one_segment_and_double_stars_for_any_number() {
    let paths = ["app", "app/a", "app/a/b", "lib"];
    let cases = [
        // `app/**` holds `app` itself and every module under it.
        (json!([["app/**"], ["lib"]]), ("lib", "app"), true),
        (json!([["app/**"], ["lib"]]), ("lib", "app/a/b"), true),
        (json!([["app/**"], ["lib"]]), ("app/a/b", "lib"), false),
        // `app/*` holds one segment below `app` only.
        (json!([["app/*"], ["lib"]]), ("lib", "app/a"), true),
        (json!([["app/*"], ["lib"]]), ("lib", "app/a/b"), false),
        (json!([["app/*"], ["lib"]]), ("lib", "app"), false),
        // `*` also works inside a segment.
        (json!([["a*"], ["lib"]]), ("lib", "app"), true),
        // `::` and `/` both separate segments, in globs and in paths.
        (json!([["app::a"], ["lib"]]), ("lib", "app/a"), true),
        (json!([["app::**"], ["lib"]]), ("lib", "app/a/b"), true),
    ];
    for (layers, (from, to), reported) in cases {
        let found = importers(&paths, &[(from, to)], json!({ "layers": layers }));
        assert_eq!(!found.is_empty(), reported, "{layers}: {from} -> {to}");
    }
}

#[test]
fn layers_keep_peers_of_a_layer_independent_unless_they_share_a_glob() {
    let paths = ["a/x", "a/y", "b/z"];
    let layers = json!({ "layers": [["a/**", "b/**"]] });
    let imports = [("a/x", "a/y"), ("a/x", "b/z")];
    assert_eq!(importers(&paths, &imports, layers), ["x".to_owned()]);
    let relaxed = json!({ "layers": [["a/**", "b/**"]], "independent": false });
    assert!(importers(&paths, &imports, relaxed).is_empty());
}

#[test]
fn layers_report_forbidden_imports_without_any_layer() {
    let forbidden = json!({ "forbidden": [{ "from": "domain", "to": "infra/**" }] });
    let paths = ["domain", "infra/db", "web"];
    let imports = [("domain", "infra/db"), ("web", "infra/db")];
    assert_eq!(
        importers(&paths, &imports, forbidden),
        ["domain".to_owned()]
    );
}

#[test]
fn layers_skip_an_import_an_ignore_entry_names() {
    let options = json!({
        "layers": [["infra"], ["domain"]],
        "ignore": [{ "from": "domain", "to": "infra", "reason": "moves down next release" }],
    });
    assert!(importers(&["infra", "domain"], &[("domain", "infra")], options).is_empty());
    let other = json!({
        "layers": [["infra"], ["domain"]],
        "ignore": [{ "from": "domain", "to": "db", "reason": "unrelated" }],
    });
    assert_eq!(
        importers(&["infra", "domain"], &[("domain", "infra")], other),
        ["domain".to_owned()]
    );
}

/// Two modules each declare a type called `name`, with these visibilities.
fn homonyms(name: &str, second_file: &str, second_private: bool) -> (World, Vec<String>) {
    let mut w = World::default();
    w.symbol("a", name, SymbolKind::Type, "a/a.ucm");
    let other = w.symbol("b", name, SymbolKind::Type, second_file);
    if second_private {
        w.private(&other);
    }
    let found = w.check(TYPE_NAMES, json!({}));
    let names = w.names(&found);
    (w, names)
}

#[test]
fn type_names_report_each_public_type_another_module_also_declares() {
    let (_, found) = homonyms("Account", "b/a.ucm", false);
    assert_eq!(found, ["Account", "Account"]);
}

#[test]
fn type_names_skip_idiomatic_names_private_types_and_code_that_is_not_production() {
    for name in ["Error", "Result", "Options", "Config"] {
        assert!(homonyms(name, "b/a.ucm", false).1.is_empty(), "{name}");
    }
    assert!(homonyms("Account", "b/a.ucm", true).1.is_empty(), "private");
    assert!(
        homonyms("Account", "b/a_test.ucm", false).1.is_empty(),
        "test"
    );
}

#[test]
fn type_names_accept_the_names_the_options_allow() {
    let mut w = World::default();
    w.symbol("a", "Account", SymbolKind::Type, "a/a.ucm");
    w.symbol("b", "Account", SymbolKind::Type, "b/a.ucm");
    assert!(
        w.check(TYPE_NAMES, json!({ "allow": ["Account"] }))
            .is_empty()
    );
}

/// Module `small` of `lines` declarations, imported by `users`.
fn small(lines: usize, users: &[&str], declares: bool) -> World {
    let mut w = World::default();
    w.module("small", None, None);
    for n in 0..lines {
        let kind = if declares {
            SymbolKind::Function
        } else {
            SymbolKind::Field
        };
        w.symbol("small", &format!("f{n}"), kind, "small/a.ucm");
    }
    for user in users {
        w.module(user, None, None);
        w.symbol(user, "run", SymbolKind::Function, &format!("{user}/a.ucm"));
        w.import(user, "small");
    }
    w
}

#[test]
fn tiny_modules_report_a_small_module_with_one_dependent_or_none_of_its_own() {
    let one = small(2, &["user"], true);
    assert_eq!(one.check(TINY, json!({ "minLines": 5 })).len(), 1);
    let empty = small(2, &["first", "second"], false);
    assert_eq!(empty.check(TINY, json!({ "minLines": 5 })).len(), 1);
}

#[test]
fn tiny_modules_skip_shared_big_and_unused_modules() {
    let shared = small(2, &["first", "second"], true);
    assert!(shared.check(TINY, json!({ "minLines": 5 })).is_empty());
    let big = small(6, &["user"], true);
    assert!(big.check(TINY, json!({ "minLines": 5 })).is_empty());
    let unused = small(2, &[], true);
    assert!(unused.check(TINY, json!({ "minLines": 5 })).is_empty());
}

#[test]
fn tiny_modules_do_not_count_a_module_that_tests_it_as_a_dependent() {
    let mut w = small(2, &["user", "check"], true);
    w.modules.retain(|m| m.path != "check");
    w.module("check", None, Some("small"));
    assert_eq!(w.check(TINY, json!({ "minLines": 5 })).len(), 1);
}
