//! Where the findings of an `edge` decision go: every source module is
//! judged, and a finding is reported at the site of the edge.

use crate::{
    rules::{found, local, scoped},
    support::World,
};
use lighthouse_checks::Checks;
use lighthouse_model::ucm::{EdgeKind, SymbolKind};
use serde_json::json;

const IMPORTS_INFRA: &str = "select: edge\nwhere: 'edge.kind == \"imports\" && edge.to.module == \"infra\"'\nmessage: 'imports infra'";

/// A project with a module `infra` that declares a function.
fn world() -> World {
    let mut w = World::default();
    w.module("infra", None, None);
    w.func("infra", "Save", "infra/a.ucm");
    w
}

fn imports_infra(scope: &str) -> Checks {
    local(scope, IMPORTS_INFRA).unwrap()
}

fn imports_infra_in(tests: &str) -> Checks {
    let scope = format!("{{ subject: project, tests: {tests} }}");
    scoped(&scope, IMPORTS_INFRA).unwrap()
}

#[test]
fn an_import_of_a_module_without_symbols_is_judged_at_its_site() {
    let mut w = world();
    w.module("root", None, None);
    w.import_in("root/lib.ucm", "root", "infra", Some(7));
    let found = found(&imports_infra("project"), &w);
    assert_eq!(found, [("root/lib.ucm".to_owned(), 7)]);
}

#[test]
fn an_import_without_a_site_is_reported_at_the_top_of_its_file() {
    let mut w = world();
    w.module("root", None, None);
    w.import_in("root/lib.ucm", "root", "infra", None);
    let found = found(&imports_infra("project"), &w);
    assert_eq!(found, [("root/lib.ucm".to_owned(), 1)]);
}

#[test]
fn an_edge_of_a_symbol_is_reported_at_its_site() {
    let plugin = local(
        "project",
        "select: edge\nwhere: 'edge.kind == \"calls\"'\nmessage: 'calls'",
    )
    .unwrap();
    let mut w = world();
    let run = w.func("domain", "Run", "domain/a.ucm");
    let save = w.func("infra", "Other", "infra/a.ucm");
    w.edge_at(EdgeKind::Calls, &run, &save, 12);
    assert_eq!(found(&plugin, &w), [("domain/a.ucm".to_owned(), 12)]);
}

#[test]
fn an_import_from_a_test_file_is_out_of_a_scope_that_excludes_tests() {
    let mut w = world();
    w.module("root", None, None);
    w.import_in("root/lib_test.ucm", "root", "infra", Some(2));
    assert!(found(&imports_infra_in("exclude"), &w).is_empty());
    assert_eq!(
        found(&imports_infra_in("include"), &w),
        [("root/lib_test.ucm".to_owned(), 2)]
    );
}

#[test]
fn layers_report_a_forbidden_import_of_a_root_that_declares_nothing() {
    let mut w = World::default();
    for path in ["app", "infra/db"] {
        w.module(path, None, None);
    }
    w.symbol("infra/db", "db", SymbolKind::Function, "infra/db/a.ucm");
    w.import_in("app/lib.ucm", "app", "infra/db", Some(3));
    let options = json!({ "forbidden": [{ "from": "app", "to": "infra/**" }] });
    assert_eq!(
        w.check("design/layers", options),
        [("app/lib.ucm".to_owned(), 3)]
    );
}
