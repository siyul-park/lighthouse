//! The bundled rules through the Rust provider: every catalog pattern that
//! has Rust examples passes them, the same intent cases as for Go.

use std::{fs, path::Path};

use lighthouse_config::Config;
use lighthouse_engine::Engine;
use lighthouse_model::Fingerprint;
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

/// Fingerprints of the `design/exported-doc` findings of a one-file crate,
/// keyed by message.
fn exported_doc_findings(source: &str) -> Vec<(String, Fingerprint)> {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), source).unwrap();
    let plugin = lighthouse_testkit::lang_rust();
    let config = Config::parse(&format!(
        "plugins = [{{ id = \"lang-rust\", path = {:?} }}, \"design\"]\n[rules]\n\"design/exported-doc\" = \"warn\"\n",
        plugin.to_str().unwrap()
    ))
    .unwrap();
    let mut registry = lighthouse_builtin::registry();
    lighthouse_rpc::register(&mut registry, &config, dir.path(), &[]).unwrap();
    let engine = Engine::new(registry, config, dir.path()).unwrap();
    let outcome = engine.check(&[], &[]).unwrap();
    assert!(outcome.incomplete.is_empty(), "{:?}", outcome.incomplete);
    outcome
        .diagnostics
        .into_iter()
        .map(|d| (d.message, d.fingerprint))
        .collect()
}

#[test]
fn fingerprints_survive_edits_above_a_finding() {
    let before = exported_doc_findings("pub fn open() {}\n\npub fn close() {}\n");
    let after = exported_doc_findings(
        "//! Crate docs.\n\nuse std::fmt;\n\n/// Documented.\npub fn added() {}\n\n\npub fn open() {}\n\npub fn close() {}\n",
    );
    assert_eq!(before.len(), 2, "{before:?}");
    for finding in &before {
        assert!(after.contains(finding), "{finding:?} not in {after:?}");
    }
}

#[test]
fn fingerprints_differ_between_symbols() {
    let found = exported_doc_findings("pub fn open() {}\n\npub fn close() {}\n");
    assert_ne!(found[0].1, found[1].1);
}
