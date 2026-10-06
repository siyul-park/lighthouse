//! The bundled rules through the Rust provider: every catalog pattern that
//! has Rust examples passes them, the same intent cases as for Go.

use std::path::Path;

use lighthouse_config::Config;
use lighthouse_plugin::Registry;

fn registry() -> Registry {
    let plugin = lighthouse_testkit::lang_rust();
    let mut registry = lighthouse_builtin::registry();
    let config = Config::parse(&format!(
        "plugins = [{{ id = \"lang-rust\", path = {:?} }}]",
        plugin.to_str().unwrap()
    ))
    .unwrap();
    lighthouse_rpc::register(&mut registry, &config, Path::new("."), &[]).unwrap();
    registry
}

#[test]
fn every_implemented_pattern_passes_its_rust_examples() {
    let failures =
        lighthouse_engine::RuleTester::new(registry, lighthouse_spec::Catalog::bundled())
            .language("rust")
            .check_all();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_design_patterns_have_rust_examples() {
    let catalog = lighthouse_spec::Catalog::bundled();
    for id in [
        "design/complexity-signal",
        "design/coupling-signal",
        "design/exported-doc",
        "design/single-use-wrapper",
    ] {
        let pattern = catalog.pattern(id).unwrap();
        let rust: Vec<_> = pattern
            .examples
            .iter()
            .filter(|e| e.language == "rust")
            .collect();
        assert!(
            rust.iter().any(|e| e.kind == lighthouse_spec::Kind::Valid)
                && rust
                    .iter()
                    .any(|e| e.kind == lighthouse_spec::Kind::Invalid),
            "{id} lacks a valid and an invalid Rust example"
        );
    }
}

#[test]
fn a_language_without_examples_is_a_failure_not_silence() {
    let failures =
        lighthouse_engine::RuleTester::new(registry, lighthouse_spec::Catalog::bundled())
            .language("cobol")
            .check_all();
    assert!(
        failures
            .iter()
            .any(|f| f.contains("no example for language `cobol`")),
        "{failures:?}"
    );
}
