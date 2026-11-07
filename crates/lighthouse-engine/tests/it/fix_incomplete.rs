//! What an incomplete analysis refuses: only the fixes that edit a file it
//! covers, or that rest on a rule which read one.

use std::{fs, path::Path};

use lighthouse_engine::{Engine, FixReport, FixRun};
use lighthouse_model::Safety;
use lighthouse_plugin::{
    Fixer, LanguageProvider, Plugin, PluginManifest, Registry, Rule, RuleManifest,
};
use lighthouse_spec::Config;
use tempfile::TempDir;

use crate::support::*;

/// What the `fake/todo` rule reads of the model besides its file.
#[derive(Clone, Copy)]
enum Reads {
    Nothing,
    /// The symbols of this file.
    File(&'static str),
    /// The symbols of the project.
    Project,
}

/// The fake plugin with the `fake/todo` rule as a node of the graph.
struct Reading(Reads);

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
        vec![Box::new(Declared(manifest, self.0))]
    }

    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        Fake(PLUGIN.clone()).fixers()
    }
}

/// `Word` with a manifest of its own, which reads what `Reads` says.
struct Declared(RuleManifest, Reads);

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
        match self.1 {
            Reads::Nothing => {}
            Reads::File(path) => {
                ctx.model.symbols_in(Path::new(path)).for_each(drop);
            }
            Reads::Project => ctx.model.symbols().for_each(drop),
        }
        Word::new("fake/todo", "TODO").check(ctx, options)
    }
}

fn fixed(reads: Reads, files: &[(&str, &str)]) -> (TempDir, FixReport) {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        fs::write(dir.path().join(name), text).unwrap();
    }
    let mut registry = Registry::default();
    registry.register(&Reading(reads)).unwrap();
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
fn a_gap_in_one_file_refuses_only_the_fix_that_edits_it() {
    let (dir, report) = fixed(
        Reads::Nothing,
        &[("a.toy", "TODO\n"), ("b.toy", "BROKEN\nTODO\n")],
    );

    assert_eq!(read(&dir, "a.toy"), "DONE\n");
    assert_eq!(read(&dir, "b.toy"), "BROKEN\nTODO\n");
    assert_eq!(report.applied.len(), 1);
    assert_eq!(report.declined.len(), 1);
    assert_eq!(report.declined[0].file.to_string_lossy(), "b.toy");
    assert!(
        report.declined[0].reason.contains("incomplete"),
        "{:?}",
        report.declined
    );
    assert!(report.declined[0].reason.contains("b.toy: cannot parse"));
}

#[test]
fn a_gap_of_the_provider_covers_the_files_of_that_provider() {
    let (dir, report) = fixed(
        Reads::Nothing,
        &[("a.toy", "TODO\nPARTIAL\n"), ("b.toy", "TODO\n")],
    );

    assert_eq!(read(&dir, "a.toy"), "TODO\nPARTIAL\n");
    assert_eq!(read(&dir, "b.toy"), "TODO\n");
    assert!(report.applied.is_empty());
    assert_eq!(report.declined.len(), 2);
    assert!(
        report.declined[0]
            .reason
            .contains("toy: the provider stopped")
    );
}

#[test]
fn a_rule_that_reads_the_project_is_refused_by_any_gap() {
    let (dir, report) = fixed(
        Reads::Project,
        &[("a.toy", "TODO\n"), ("b.toy", "BROKEN\n")],
    );

    assert_eq!(read(&dir, "a.toy"), "TODO\n");
    assert!(report.applied.is_empty());
    assert_eq!(report.declined.len(), 1);
}

#[test]
fn a_rule_that_reads_a_file_is_refused_only_by_a_gap_in_it() {
    let files = [
        ("a.toy", "TODO\n"),
        ("b.toy", "BROKEN\n"),
        ("c.toy", "fn c\n"),
    ];
    let (dir, report) = fixed(Reads::File("b.toy"), &files);
    let (other, applied) = fixed(Reads::File("c.toy"), &files);

    assert_eq!(read(&dir, "a.toy"), "TODO\n");
    assert!(report.declined[0].reason.contains("b.toy: cannot parse"));
    assert_eq!(read(&other, "a.toy"), "DONE\n", "{:?}", applied.declined);
    assert!(applied.declined.is_empty(), "{:?}", applied.declined);
}
