//! Whom a finding that turns up in a file the round did not change is charged
//! to: by the reach of its rule, nobody, the changed files it has an edge to,
//! or all of them.

use std::{fs, path::Path};

use lighthouse_engine::{Engine, FixReport, FixRun};
use lighthouse_model::{Diagnostic, Fingerprint, Options, Position, Reach, Safety, Severity, Span};
use lighthouse_plugin::{
    Caching, Context, Error as PluginError, Fixer, LanguageProvider, Plugin, PluginManifest,
    Registry, Rule, RuleManifest,
};
use lighthouse_spec::Config;

use crate::support::*;

/// Reports an error in `ghost.toy` for as long as `a.toy` declares `fn z`: a
/// finding that appears in a file nobody edited. It declares `reach`.
struct Ghost {
    manifest: RuleManifest,
}

impl Ghost {
    fn new(reach: Option<Reach>) -> Self {
        let mut manifest = Word::new("fake/ghost", "GHOST").manifest().clone();
        manifest.caching = reach.map(|reach| Caching {
            reach,
            positions: false,
            revision: "0".to_owned(),
            tools: Vec::new(),
        });
        Self { manifest }
    }
}

impl Rule for Ghost {
    fn manifest(&self) -> &RuleManifest {
        &self.manifest
    }

    fn validate(&self, _: &Options) -> Result<(), PluginError> {
        Ok(())
    }

    fn check(&self, ctx: &Context, _: &Options) -> Result<Vec<Diagnostic>, PluginError> {
        let (file, _) = ctx
            .file
            .ok_or_else(|| PluginError::Failed("no file".into()))?;
        let declared = ctx
            .project
            .symbols_in(Path::new("a.toy"))
            .any(|symbol| symbol.name == "z");
        if file.path != Path::new("ghost.toy") || !declared {
            return Ok(Vec::new());
        }
        Ok(vec![Diagnostic::new(
            "fake/ghost",
            Severity::Error,
            "z is declared".to_owned(),
            &file.path,
            Span {
                start: Position { line: 1, col: 1 },
                end: Position { line: 1, col: 1 },
            },
            Fingerprint::new("fake/ghost", "ghost.toy", ""),
        )])
    }
}

/// The toy plugin with the rule above, and a fix that declares `z`.
struct Haunted(Option<Reach>);

impl Plugin for Haunted {
    fn manifest(&self) -> &PluginManifest {
        &PLUGIN
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Toy(toy()))]
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![
            Box::new(Word::new("fake/todo", "TODO")),
            Box::new(Ghost::new(self.0)),
        ]
    }

    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        vec![Box::new(rewrite("fake/declare", "fn z", Safety::Safe, &[]))]
    }
}

/// Fixes the `TODO` of `a.toy` into the declaration of `z`; `ghost.toy` is
/// where the finding appears, and `calls` tells whether its function calls `z`.
fn haunted(reach: Option<Reach>, calls: bool) -> (tempfile::TempDir, FixReport) {
    let dir = project("fn a\nTODO\n");
    let ghost = if calls { "fn g\ncall z\n" } else { "fn g\n" };
    fs::write(dir.path().join("ghost.toy"), ghost).unwrap();
    let mut registry = Registry::default();
    registry.register(&Haunted(reach)).unwrap();
    let config = Config::parse_inline(
        "plugins = [\"fake\"]\n[rules]\n\"fake/todo\" = \"error\"\n\"fake/ghost\" = \"error\"\n",
    )
    .unwrap();
    let engine = Engine::new(
        registry,
        config,
        lighthouse_test_support::bundled(),
        dir.path(),
    )
    .unwrap();
    let report = engine
        .fix(&plan("fake/declare", Safety::Safe), &FixRun::default())
        .unwrap();
    (dir, report)
}

/// Whether the fix of `a.toy` stands.
fn is_kept(dir: &tempfile::TempDir) -> bool {
    text(dir) == "fn a\nfn z\n"
}

#[test]
fn a_local_finding_that_turns_up_in_an_unchanged_file_charges_nobody() {
    let (dir, report) = haunted(Some(Reach::Local), true);

    assert!(is_kept(&dir), "{:?}", report.notes);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
}

#[test]
fn a_global_finding_that_turns_up_in_an_unchanged_file_charges_the_changed_files() {
    let (dir, report) = haunted(Some(Reach::Global), false);

    assert!(!is_kept(&dir));
    assert!(
        report.notes[0].contains("fake/ghost in ghost.toy"),
        "{:?}",
        report.notes
    );
}

#[test]
fn a_rule_that_declares_no_reach_is_global() {
    let (dir, _) = haunted(None, false);

    assert!(!is_kept(&dir));
}

#[test]
fn a_neighbors_finding_is_charged_to_the_changed_files_that_hold_the_neighbours() {
    let (far, unrelated) = haunted(Some(Reach::Neighbors), false);
    let (near, _) = haunted(Some(Reach::Neighbors), true);

    assert!(is_kept(&far), "{:?}", unrelated.notes);
    assert!(!is_kept(&near));
}
