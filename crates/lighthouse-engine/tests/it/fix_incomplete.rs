//! What an incomplete analysis refuses: a fix, whatever file the gap is in.

use std::fs;

use lighthouse_engine::{Engine, FixReport, FixRun};
use lighthouse_model::Safety;
use lighthouse_plugin::{
    Fixer, LanguageProvider, Plugin, PluginManifest, Registry, Rule, RuleManifest,
};
use lighthouse_spec::Config;
use tempfile::TempDir;

use crate::support::*;

/// The fake plugin with the `fake/todo` rule as a node of the graph.
struct Reading;

impl Plugin for Reading {
    fn manifest(&self) -> &PluginManifest {
        &PLUGIN
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Toy(toy()))]
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        let mut manifest: RuleManifest = Word::new("fake/todo", "TODO").manifest().clone();
        manifest.revision = Some("0".to_owned());
        vec![Box::new(Declared(manifest))]
    }

    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        Fake(PLUGIN.clone()).fixers()
    }
}

/// `Word` with a manifest of its own.
struct Declared(RuleManifest);

impl Rule for Declared {
    fn manifest(&self) -> &RuleManifest {
        &self.0
    }

    fn validate(
        &self,
        options: &lighthouse_model::Options,
    ) -> Result<(), lighthouse_plugin::Error> {
        Word::new("fake/todo", "TODO").validate(options)
    }

    fn check(
        &self,
        ctx: &lighthouse_plugin::Context,
        options: &lighthouse_model::Options,
    ) -> Result<Vec<lighthouse_model::Diagnostic>, lighthouse_plugin::Error> {
        Word::new("fake/todo", "TODO").check(ctx, options)
    }
}

fn fixed(files: &[(&str, &str)]) -> (TempDir, FixReport) {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        fs::write(dir.path().join(name), text).unwrap();
    }
    let mut registry = Registry::default();
    registry.register(&Reading).unwrap();
    let config =
        Config::parse_inline("plugins = [\"fake\"]\n[rules]\n\"fake/todo\" = \"error\"\n").unwrap();
    let engine = Engine::new(
        registry,
        config,
        lighthouse_test_support::bundled(),
        dir.path(),
    )
    .unwrap();
    let report = engine
        .fix(&plan("fake/to-done", Safety::Safe), &FixRun::default())
        .unwrap();
    (dir, report)
}

fn read(dir: &TempDir, name: &str) -> String {
    fs::read_to_string(dir.path().join(name)).unwrap()
}

#[test]
fn a_rule_that_reads_the_project_is_refused_by_any_gap() {
    let (dir, report) = fixed(&[("a.toy", "TODO\n"), ("b.toy", "BROKEN\n")]);

    assert_eq!(read(&dir, "a.toy"), "TODO\n");
    assert!(report.applied.is_empty());
    assert_eq!(report.declined.len(), 1);
}
