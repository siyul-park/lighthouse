//! The order decisions of the bundled catalog held to each other over the
//! shapes of declarations, in Go and Rust: the contradictions and the gaps it
//! reports, and what each search costs cold and warm. Not a check: run it in
//! release with `--ignored --nocapture`.

use std::time::Instant;

use lighthouse_engine::{Basis, Cell, Oracle};
use lighthouse_graph::{Graph, Options};
use lighthouse_logic::Tier;
use lighthouse_spec::{Config, Decision};
use lighthouse_test_support::{bundled, lang_go, lang_rust, pooled, registry};
use serde_json::{Map, Value};

/// The decisions whose fix is a move: the order decisions.
const ORDER: [&str; 4] = [
    "design/declaration-groups",
    "design/top-down-order",
    "design/vertical-distance",
    "testing/file-layout",
];

#[test]
#[ignore = "a measurement: run in release with --nocapture"]
fn the_order_decisions_of_the_bundled_catalog() {
    let (oracle, go) = oracle();
    let catalog = bundled();
    for language in ["rust", "go"] {
        if language == "go" && !go {
            continue;
        }
        for bound in [2, 3] {
            let decisions = ORDER
                .iter()
                .map(|id| catalog.decision(id).unwrap())
                .filter(|decision| decision.applies_to(language))
                .map(|decision| (decision, Map::new()))
                .collect();
            let cell = Cell {
                language,
                decisions,
            };
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("graph.db");
            let open = || Graph::open(&db, Options::default()).unwrap();
            let cold = Instant::now();
            let found = oracle.search(&mut open(), &cell, bound);
            let cold = cold.elapsed();
            let warm = Instant::now();
            let again = oracle.search(&mut open(), &cell, bound);
            let warm = warm.elapsed();
            println!(
                "REPORT {language} n={bound} cold={cold:?} warm={warm:?} ran={:?} warm_ran={:?}",
                found.ran, again.ran
            );
            println!("{}", serde_json::to_string_pretty(&found.verdict).unwrap());
            assert_eq!(found.verdict, again.verdict);
        }
    }
}

#[test]
#[ignore = "slow: run by `make decisions` in release"]
fn types_that_hold_each_other_in_a_cycle_are_not_ordered_by_top_down_order() {
    let (oracle, _) = oracle();
    let catalog = bundled();
    let decision = catalog.decision("design/top-down-order").unwrap();
    let cell = Cell {
        language: "rust",
        decisions: vec![(decision, Map::new())],
    };
    let mut graph = Graph::in_memory(Options::default()).unwrap();

    let found = oracle.search(&mut graph, &cell, 3);

    assert!(
        found.verdict.single.is_empty(),
        "{:?}",
        found.verdict.single
    );
    assert!(
        matches!(found.verdict.basis, Basis::Bounded(_)),
        "{:?}",
        found.verdict.basis
    );
}

/// The gate of the catalog: the order decisions of the bundled catalog, at
/// their default options, hold together over every set of up to three
/// declarations in Go and Rust, and the languages without a renderer say so.
/// Run with `--nocapture` for the report.
#[test]
#[ignore = "slow: run by `make decisions` in release"]
fn the_bundled_catalog_has_no_contradiction_up_to_three_declarations() {
    let (oracle, go) = oracle();
    let catalog = bundled();
    let cel = lighthouse_test_support::cel();
    let orders = |language: &str| -> Vec<(&Decision, Map<String, Value>)> {
        catalog
            .decisions()
            .filter(|d| d.automated() && d.applies_to(language))
            .filter(|d| lighthouse_logic::tier(&cel, d).tier == Tier::O)
            .map(|d| (d, Map::new()))
            .collect()
    };
    for language in ["rust", "go", "python", "typescript"] {
        let rendered = matches!(language, "rust" | "go");
        if language == "go" && !go {
            continue;
        }
        let decisions = orders(language);
        let mut names: Vec<&str> = decisions.iter().map(|(d, _)| d.id()).collect();
        names.sort_unstable();
        let expected: &[&str] = if rendered {
            &ORDER
        } else {
            &["testing/file-layout"]
        };
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        assert_eq!(names, expected, "{language}: the order decisions in force");
        let cell = Cell {
            language,
            decisions,
        };
        let mut graph = Graph::in_memory(Options::default()).unwrap();

        let found = oracle.search(&mut graph, &cell, 3).verdict;

        println!(
            "GATE {language} n=3 decisions={names:?} contradictions={} single={} basis={}",
            found.contradictions.len(),
            found.single.len(),
            match &found.basis {
                Basis::Bounded(b) => format!("bounded({})", b.declarations),
                Basis::Incomplete(why) => format!(
                    "incomplete({})",
                    why.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            }
        );
        assert!(found.contradictions.is_empty(), "{language}: {found:?}");
        assert!(found.single.is_empty(), "{language}: {found:?}");
        match (&found.basis, rendered) {
            (Basis::Bounded(b), true) => assert_eq!(b.declarations, 3),
            (Basis::Incomplete(why), false) => {
                assert!(
                    why.iter().any(|w| w.to_string().contains("no renderer")),
                    "{why:?}"
                );
            }
            (basis, _) => panic!("{language}: unexpected basis {basis:?}"),
        }
    }
}

/// An oracle over the bundled catalog and the plugins of Rust and, when a Go
/// toolchain runs, Go; whether Go is among them.
fn oracle() -> (Oracle<'static>, bool) {
    let mut plugins = vec![format!(
        "{{ id = \"lang-rust\", path = {:?} }}",
        lang_rust().display().to_string()
    )];
    let go = lang_go();
    if let Some(go) = &go {
        plugins.push(format!(
            "{{ id = \"lang-go\", path = {:?} }}",
            go.display().to_string()
        ));
    }
    let config = Config::parse_inline(&format!("plugins = [{}]", plugins.join(", "))).unwrap();
    let config: &'static Config = Box::leak(Box::new(config));
    (
        Oracle::new(pooled(registry, config), bundled()),
        go.is_some(),
    )
}
