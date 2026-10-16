//! The reach of modules and symbols: the graph algorithm on hand-made
//! projects, the facts `module.stability` and `reach(n)` read by cel checks,
//! and the graphs of what the Go and Rust providers index.

use std::{fs, path::Path};

use lighthouse_checks::metrics::{
    Measures,
    propagation::{System, modules, symbols},
};
use lighthouse_engine::Engine;
use lighthouse_model::{EdgeKind, Flow, FlowKind, Symbol, SymbolKind};
use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Config};
use serde_json::json;

use crate::{
    func_facts::{flagged, flagged_in},
    support::World,
};

const GENERATED: &str = "g/gen.ucm";

/// A module `name` with one function of the same name.
fn node(w: &mut World, name: &str) -> Symbol {
    w.module(name, None, None);
    w.func(name, name, &format!("{name}/a.ucm"))
}

/// Modules `names`, each calling the ones its edges name.
fn world(names: &[&str], uses: &[(&str, &str)]) -> World {
    let mut w = World::default();
    let found: Vec<Symbol> = names.iter().map(|n| node(&mut w, n)).collect();
    for (from, to) in uses {
        let at = |name: &str| found.iter().find(|s| s.name == name).unwrap().clone();
        w.edge(EdgeKind::Calls, &at(from), &at(to));
    }
    w
}

fn reach_of(w: &World, name: &str) -> (u32, u32, u32, u32) {
    let system = modules(&w.project());
    let s = system.of(name).unwrap();
    (s.ca, s.ce, s.vfi, s.vfo)
}

#[test]
fn a_chain_reaches_everything_downstream() {
    let w = world(&["a", "b", "c"], &[("a", "b"), ("b", "c")]);
    let system = modules(&w.project());
    assert_eq!(system.of("a").unwrap().vfo, 2);
    assert_eq!(system.of("c").unwrap().vfi, 2);
    assert_eq!(reach_of(&w, "b"), (1, 1, 1, 1));
    assert!((system.propagation_cost - 6.0 / 9.0).abs() < 1e-12);
    assert!(system.cycles.is_empty());
}

#[test]
fn a_diamond_counts_the_bottom_once() {
    let w = world(
        &["top", "left", "right", "bottom"],
        &[
            ("top", "left"),
            ("top", "right"),
            ("left", "bottom"),
            ("right", "bottom"),
        ],
    );
    assert_eq!(reach_of(&w, "top"), (0, 2, 0, 3));
    assert_eq!(reach_of(&w, "bottom"), (2, 0, 3, 0));
}

#[test]
fn the_largest_cycle_is_the_core_and_a_tie_goes_to_the_smallest_name() {
    let w = world(
        &["a", "b", "c", "d"],
        &[("a", "b"), ("b", "a"), ("c", "d"), ("d", "c"), ("b", "c")],
    );
    let system = modules(&w.project());
    let found: Vec<_> = ["a", "b", "c", "d"]
        .map(|n| system.of(n).unwrap())
        .into_iter()
        .map(|s| (s.cyclic, s.component.as_str(), s.core))
        .collect();
    assert_eq!(
        found,
        [
            (2, "a", true),
            (2, "a", true),
            (2, "c", false),
            (2, "c", false)
        ]
    );
    assert_eq!(system.cycles, [["a", "b"], ["c", "d"]]);
    assert_eq!(reach_of(&w, "a"), (1, 1, 1, 3));
    assert!((system.core_share - 0.5).abs() < 1e-12);
}

#[test]
fn a_self_loop_is_no_cycle() {
    let w = world(&["a"], &[("a", "a")]);
    let system = modules(&w.project());
    let s = system.of("a").unwrap();
    assert_eq!((s.cyclic, s.vfo, s.ca, s.ce, s.core), (1, 0, 0, 0, false));
    assert!(system.cycles.is_empty());
}

#[test]
fn an_empty_project_has_no_cost_and_no_core() {
    for system in [modules, symbols].map(|measure| measure(&World::default().project())) {
        assert!(system.nodes.is_empty());
        assert_eq!((system.propagation_cost, system.core_share), (0.0, 0.0));
    }
}

/// A design structure matrix of six nodes, `x -> y` meaning x uses y:
///
/// ```text
/// a -> b -> c -> a     the cycle {a, b, c}
/// c -> d -> e          the chain below it
/// f -> e
/// ```
///
/// Each of a, b and c reaches the other two of the cycle and d and e: 4. d
/// reaches e, f reaches e and e nothing, so the visible fan-out sums to
/// 4 + 4 + 4 + 1 + 0 + 1 = 14 and, with the diagonal, the propagation cost is
/// (14 + 6) / 36 = 5/9. The core is the cycle: half of the nodes.
#[test]
fn the_propagation_cost_of_a_six_node_matrix_is_worked_out_by_hand() {
    let w = world(
        &["a", "b", "c", "d", "e", "f"],
        &[
            ("a", "b"),
            ("b", "c"),
            ("c", "a"),
            ("c", "d"),
            ("d", "e"),
            ("f", "e"),
        ],
    );
    let system = modules(&w.project());
    let fan_out: Vec<u32> = system.stability.iter().map(|s| s.vfo).collect();
    let fan_in: Vec<u32> = system.stability.iter().map(|s| s.vfi).collect();
    assert_eq!(fan_out, [4, 4, 4, 1, 0, 1]);
    assert_eq!(fan_in, [2, 2, 2, 3, 5, 0]);
    assert!((system.propagation_cost - 20.0 / 36.0).abs() < 1e-12);
    assert!((system.core_share - 0.5).abs() < 1e-12);
}

#[test]
fn an_import_joins_modules_that_use_nothing_of_each_other() {
    let mut w = world(&["a", "b"], &[]);
    w.import("a", "b");
    assert_eq!(reach_of(&w, "a"), (0, 1, 0, 1));
    assert_eq!(reach_of(&w, "b"), (1, 0, 1, 0));
}

#[test]
fn tests_and_generated_code_are_outside_the_graphs() {
    let mut w = world(&["a", "b"], &[("a", "b")]);
    w.files.insert(GENERATED.to_owned(), true);
    let gen_fn = w.func("g", "gen", GENERATED);
    w.module("g", None, None);
    let a = w.symbol("a", "again", SymbolKind::Function, "a/a.ucm");
    w.edge(EdgeKind::Calls, &a, &gen_fn);
    w.module("a_test", None, Some("a"));
    let check = w.func("a_test", "check", "a_test/x_test.ucm");
    w.edge(EdgeKind::Calls, &check, &a);
    let project = w.project();
    let (by_module, by_symbol) = (modules(&project), symbols(&project));
    assert_eq!(by_module.nodes, ["a", "b"]);
    assert!(by_symbol.of(gen_fn.id.as_str()).is_none());
    assert!(by_symbol.of(check.id.as_str()).is_none());
    assert_eq!(
        by_module.of("a").unwrap().ce,
        1,
        "the call into g is dropped"
    );
}

#[test]
fn a_module_nobody_touches_has_no_instability() {
    let w = world(&["a", "b", "c"], &[("a", "b")]);
    let isolated = |condition: &str| -> Vec<String> {
        flagged(&w, "module", condition, &json!({}))
            .into_iter()
            .map(|(file, _)| file)
            .collect()
    };
    assert_eq!(
        isolated("module.stability != null && module.stability.instability == null"),
        ["c/a.ucm"]
    );
    assert_eq!(
        isolated("module.stability != null && module.stability.instability == 1.0"),
        ["a/a.ucm"]
    );
    assert_eq!(
        isolated("module.stability != null && module.stability.vfo_share == 0.3333"),
        ["a/a.ucm"]
    );
}

#[test]
fn a_test_module_has_no_stability() {
    let mut w = world(&["a"], &[]);
    w.module("a_test", None, Some("a"));
    w.func("a_test", "check", "a_test/x_test.ucm");
    let found = flagged_in(
        &w,
        "module",
        ", tests: include",
        "module.stability == null",
        &json!({}),
    );
    assert_eq!(
        found.into_iter().map(|(f, _)| f).collect::<Vec<_>>(),
        ["a_test/x_test.ucm"]
    );
}

#[test]
fn reach_of_a_test_function_is_not_measured() {
    let mut w = world(&["a"], &[]);
    w.module("a_test", None, Some("a"));
    w.func("a_test", "check", "a_test/x_test.ucm");
    let found = flagged_in(
        &w,
        "function",
        ", tests: include",
        "!reach(func).measured",
        &json!({}),
    );
    assert_eq!(w.names(&found.into_iter().collect::<Vec<_>>()), ["check"]);
}

#[test]
fn reach_reads_the_symbol_graph_across_modules() {
    let w = world(&["a", "b", "c"], &[("a", "b"), ("b", "c")]);
    let names = |condition: &str| -> Vec<String> {
        let found: Vec<_> = flagged(&w, "function", condition, &json!({}))
            .into_iter()
            .collect();
        w.names(&found)
    };
    assert_eq!(names("reach(func).vfi == 2"), ["c"]);
    assert_eq!(names("reach(func).vfo == 2 && reach(func).measured"), ["a"]);
    assert_eq!(
        names("reach(func).ca == 1 && reach(func).ce == 1 && reach(func).cyclic == 1"),
        ["b"]
    );
    assert_eq!(names("reach(func).vfi_share == 0.6667"), ["c"]);
}

#[test]
fn reach_of_a_symbol_that_is_no_node_is_not_measured() {
    let mut w = world(&["a"], &[]);
    w.symbol("a", "LIMIT", SymbolKind::Const, "a/a.ucm");
    let found = flagged(
        &w,
        "symbol",
        "!reach(symbol).measured && reach(symbol).vfi == 0 && reach(symbol).cyclic == 0",
        &json!({}),
    );
    assert_eq!(found.len(), 1);
}

/// The graph of modules of `files` as the provider `id` at `plugin` indexes
/// them.
fn indexed(id: &str, plugin: &Path, files: &[(&str, &str)]) -> System {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let mut registry: Registry = lighthouse_checks::registry();
    let config = Config::parse_inline(&format!(
        "plugins = [{{ id = {id:?}, path = {:?} }}, \"design\"]",
        plugin.to_str().unwrap()
    ))
    .unwrap();
    lighthouse_rpc::register(&mut registry, &config, Path::new("."), &[]).unwrap();
    let outcome = Engine::new(registry, config, Catalog::bundled(), dir.path())
        .unwrap()
        .check(&[], &[])
        .unwrap();
    modules(&outcome.project)
}

#[test]
fn the_go_provider_indexes_packages_that_reach_each_other() {
    let Some(plugin) = lighthouse_test_support::lang_go() else {
        return;
    };
    let system = indexed(
        "lang-go",
        &plugin,
        &[
            ("go.mod", "module example.com/app\n"),
            (
                "store/store.go",
                "package store\n\n// Put stores.\nfunc Put() {}\n",
            ),
            (
                "cmd/main.go",
                "package main\n\nimport \"example.com/app/store\"\n\nfunc main() { store.Put() }\n",
            ),
        ],
    );
    let store = system
        .of("store")
        .unwrap_or_else(|| panic!("{:?}", system.nodes));
    assert_eq!((store.ca, store.vfi), (1, 1));
    assert_eq!(system.of("cmd").unwrap().vfo, 1);
}

#[test]
fn the_rust_provider_indexes_modules_that_reach_each_other() {
    let plugin = lighthouse_test_support::lang_rust();
    let system = indexed(
        "lang-rust",
        &plugin,
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            ),
            (
                "src/lib.rs",
                "mod util;\n\n/// Runs.\npub fn run() {\n    util::help();\n}\n",
            ),
            ("src/util.rs", "pub fn help() {}\n"),
        ],
    );
    assert!(!system.nodes.is_empty(), "{:?}", system.nodes);
    assert!(
        system.stability.iter().any(|s| s.ca == 1 && s.vfi == 1),
        "{system:?}"
    );
}

#[test]
fn measures_weigh_a_function_the_way_the_hotspots_do() {
    let mut w = World::default();
    let f = w.func("m", "f", "m/a.ucm");
    w.summarize(&f, 3, &[Flow::new(FlowKind::If, 0); 2]);
    let project = w.project();
    let measured = Measures::of(&project, &f, project.function(&f.id).unwrap());
    assert_eq!(
        (
            measured.lines,
            measured.statements,
            measured.cyclomatic,
            measured.cognitive
        ),
        (1, 3, 3, 2)
    );
}
