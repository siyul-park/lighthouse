//! Fix previews over the toy language: what a fix run leaves for a hand.

use std::{collections::BTreeSet, fs, path::Path};

use lighthouse_engine::{Engine, FixPlan, FixRun, Needs};
use lighthouse_model::{
    Safety,
    ucm::{EditOp, FixOutcome},
};
use lighthouse_plugin::{
    Error as PluginError, FixRequest, Fixer, FixerManifest, LanguageProvider, Plugin,
    PluginManifest, Registry, Rule,
};
use lighthouse_spec::Config;

use crate::support::*;

#[test]
fn a_dry_run_previews_an_unsafe_fix_and_writes_nothing() {
    let dir = project("fn a\nTODO here\n");
    let engine = engine(&dir, "");
    let run = FixRun {
        dry_run: true,
        ..FixRun::default()
    };

    let report = engine
        .fix(&plan("fake/to-done", Safety::Unsafe), &run)
        .unwrap();

    assert_eq!(report.previews.len(), 1, "{:?}", report.declined);
    let preview = &report.previews[0].preview;
    assert_eq!(preview.safety, Safety::Unsafe);
    assert!(preview.files[0].hunks.contains("+DONE here"));
    assert_eq!(text(&dir), "fn a\nTODO here\n", "nothing was written");
}

#[test]
fn a_fix_that_cannot_be_proposed_has_no_preview() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let run = FixRun::default();

    let none = engine.fix(&FixPlan::default(), &run).unwrap();
    assert!(none.previews.is_empty(), "a rule without a fix");
    let unknown = engine
        .fix(&plan("fake/missing", Safety::Unsafe), &run)
        .unwrap();
    assert!(
        unknown.previews.is_empty(),
        "a fixer that is not registered"
    );
}

#[test]
fn fix_plan_needs_a_program_only_for_fixers_that_run_one() {
    let plan = FixPlan::from_catalog(
        lighthouse_test_support::bundled(),
        lighthouse_test_support::bundled(),
    );
    let needs = |rule: &str| plan.get(rule).map(|binding| binding.needs.clone());
    assert_eq!(needs("design/declaration-groups"), Some(Needs::Nothing));
    assert_eq!(needs("nope/nope"), None);
}

/// Replaces the `TODO` with `DONE`, but with `BAD` in `b.toy`: an unsafe fix
/// that the checks find worse.
struct Spoiling(FixerManifest);

impl Fixer for Spoiling {
    fn manifest(&self) -> &FixerManifest {
        &self.0
    }

    fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
        let broken = request.finding.file == Path::new("b.toy");
        Ok(FixOutcome::Proposed {
            description: "replace".to_owned(),
            ops: vec![EditOp::Replace {
                file: request.finding.file.clone(),
                span: request.finding.span,
                text: if broken { "BAD" } else { "DONE" }.to_owned(),
            }],
            safety: Safety::Unsafe,
        })
    }
}

struct Spoiler;

impl Plugin for Spoiler {
    fn manifest(&self) -> &PluginManifest {
        &PLUGIN
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Toy(toy()))]
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        vec![
            Box::new(Word::new("fake/todo", "TODO")),
            Box::new(Word::new("fake/bad", "BAD")),
        ]
    }

    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        vec![Box::new(Spoiling(FixerManifest {
            id: "fake/spoiling".to_owned(),
            requires: Vec::new(),
        }))]
    }
}

#[test]
fn one_fix_that_the_checks_find_worse_among_many_costs_few_checks() {
    const FILES: usize = 32;
    let dir = tempfile::tempdir().unwrap();
    for n in 0..FILES {
        let name = if n == 5 {
            "b".to_owned()
        } else {
            format!("f{n:02}")
        };
        fs::write(dir.path().join(format!("{name}.toy")), "fn x\nTODO\nTODO\n").unwrap();
    }
    let mut registry = Registry::default();
    registry.register(&Spoiler).unwrap();
    let config = Config::parse_inline(
        "plugins = [\"fake\"]\n[rules]\n\"fake/todo\" = \"error\"\n\"fake/bad\" = \"error\"\n",
    )
    .unwrap();
    let engine = Engine::new(
        registry,
        config,
        lighthouse_test_support::bundled(),
        dir.path(),
    )
    .unwrap();
    let run = FixRun {
        dry_run: true,
        ..FixRun::default()
    };

    let report = engine
        .fix(&plan("fake/spoiling", Safety::Unsafe), &run)
        .unwrap();

    // Every fix but the two of `b.toy` is previewed, as when they were halved
    // down to single fixes.
    let files: BTreeSet<_> = report
        .previews
        .iter()
        .flat_map(|p| p.preview.files.iter().map(|f| f.path.clone()))
        .collect();
    assert_eq!(
        report.previews.len(),
        2 * (FILES - 1),
        "{:?}",
        report.declined
    );
    assert_eq!(files.len(), FILES - 1);
    assert!(!files.iter().any(|f| f.ends_with("b.toy")), "{files:?}");
    // The check of the project, the trial of all fixes, the trial of the files
    // it did not spoil, and one for each of the two fixes it did.
    let checks: usize = checks_per_round(&report).iter().map(|(_, n)| n).sum();
    assert!(checks <= 5, "{checks} {:?}", report.timings.checks);
}
