//! Examples run against registries that share one start of each plugin find
//! what examples run against a plugin started for each of them find: the
//! plugins keep nothing between requests that a later example could see.

use std::path::Path;

use lighthouse_engine::RuleTester;
use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Config};

/// How many of the decisions that apply are left out between two that are
/// run, for a plugin whose start is slow: a plugin started for each example
/// makes the whole catalog take minutes.
const GO_STRIDE: usize = 4;

/// What running the examples of `decisions` found: failures, conflicts and
/// skipped examples, by decision.
fn found(tester: &RuleTester<'_>, decisions: &[&lighthouse_spec::Decision]) -> Vec<String> {
    decisions
        .iter()
        .flat_map(|decision| {
            let tested = tester.tested(decision);
            let mut lines = vec![format!("== {}", decision.id())];
            lines.extend(tested.failures.iter().map(|f| format!("failure {f}")));
            lines.extend(tested.conflicts.iter().map(|c| format!("conflict {c}")));
            lines.extend(
                tester
                    .skipped(decision)
                    .into_iter()
                    .map(|s| format!("skipped {s}")),
            );
            lines
        })
        .collect()
}

/// The decisions with examples that run in `language`, every `stride`th.
fn applying<'c>(
    catalog: &'c Catalog,
    language: &str,
    stride: usize,
) -> Vec<&'c lighthouse_spec::Decision> {
    catalog
        .decisions()
        .filter(|d| d.automated() && d.applies_to(language))
        .step_by(stride)
        .collect()
}

fn fresh(config: &Config) -> impl Fn() -> Registry + '_ {
    move || {
        let mut registry = lighthouse_test_support::registry();
        lighthouse_rpc::register(&mut registry, config, Path::new("."), Path::new("."), &[])
            .unwrap();
        registry
    }
}

fn same(catalog: &Catalog, config: &Config, language: &str, stride: usize) {
    let decisions = applying(catalog, language, stride);
    assert!(!decisions.is_empty(), "no decision applies to {language}");
    let per_example = RuleTester::new(fresh(config), catalog, lighthouse_test_support::bundled())
        .language(language)
        .trusted(true);
    let pool = RuleTester::new(
        lighthouse_test_support::pooled(lighthouse_test_support::registry, config),
        catalog,
        lighthouse_test_support::bundled(),
    )
    .language(language)
    .trusted(true);
    assert_eq!(found(&pool, &decisions), found(&per_example, &decisions));
}

#[test]
fn a_pooled_rust_run_over_the_catalog_equals_a_run_with_a_plugin_per_example() {
    let plugin = lighthouse_test_support::lang_rust();
    let config = Config::parse_inline(&format!(
        "plugins = [{{ id = \"lang-rust\", path = {:?} }}]",
        plugin.to_str().unwrap()
    ))
    .unwrap();
    same(lighthouse_test_support::bundled(), &config, "rust", 1);
}

#[test]
fn a_pooled_go_run_equals_a_run_with_a_plugin_per_example() {
    let Some(plugin) = lighthouse_test_support::lang_go() else {
        return;
    };
    let config = Config::parse_inline(&format!(
        "plugins = [{{ id = \"lang-go\", path = {:?} }}]",
        plugin.to_str().unwrap()
    ))
    .unwrap();
    same(lighthouse_test_support::full(), &config, "go", GO_STRIDE);
}
