//! Whom a finding that turns up in a file the round did not change is charged
//! to: by the reach of its rule, nobody, the changed files it has an edge to,
//! or all of them.

use lighthouse_model::ucm::Model;
use std::{fs, path::Path};

use lighthouse_engine::{Engine, FixBinding, FixReport, FixRun, FixedBy, Needs};
use lighthouse_model::{
    Diagnostic, Fingerprint, LineIndex, Options, Reach, Safety, Severity, Span,
    ucm::{EditOp, FixOutcome},
};
use lighthouse_plugin::{
    Caching, Context, Error as PluginError, FixDecision, FixRequest, Fixer, FixerManifest,
    LanguageProvider, Plugin, PluginManifest, Registry, Rule, RuleManifest,
};
use lighthouse_spec::Config;

use crate::support::*;

/// Reports an error in `ghost.toy` for as long as `a.toy` declares `fn z`: a
/// finding that appears in a file nobody edited, at the `BOO` it holds if it
/// does. It declares `reach`.
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
        let (file, text) = ctx
            .file
            .ok_or_else(|| PluginError::Failed("no file".into()))?;
        let declared = ctx
            .project
            .symbols_in(Path::new("a.toy"))
            .any(|symbol| symbol.name == "z");
        if file.path != Path::new("ghost.toy") || !declared || !text.contains("BOO") {
            return Ok(Vec::new());
        }
        let index = LineIndex::new(text);
        let at = text.find("BOO").unwrap_or_default();
        let len = 3;
        Ok(vec![Diagnostic::new(
            "fake/ghost",
            Severity::Error,
            "z is declared".to_owned(),
            &file.path,
            Span {
                start: index.position(at),
                end: index.position(at + len),
            },
            Fingerprint::new("fake/ghost", "ghost.toy", ""),
        )])
    }
}

/// Cannot judge `ghost.toy` for as long as `a.toy` declares `fn z`: a gap that
/// appears in a file nobody edited.
struct Murk {
    manifest: RuleManifest,
}

impl Rule for Murk {
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
        if file.path == Path::new("ghost.toy") && declared {
            return Err(PluginError::Incomplete("the file is murky".to_owned()));
        }
        Ok(Vec::new())
    }
}

/// The toy plugin with the rule above, and a fix that declares `z`; with `murky`,
/// the rule leaves a gap in `ghost.toy` instead of a finding.
struct Haunted(Option<Reach>, bool);

impl Plugin for Haunted {
    fn manifest(&self) -> &PluginManifest {
        &PLUGIN
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Toy(toy()))]
    }

    fn rules(&self) -> Vec<Box<dyn Rule>> {
        let ghost: Box<dyn Rule> = if self.1 {
            let mut manifest = Word::new("fake/ghost", "GHOST").manifest().clone();
            manifest.caching = self.0.map(|reach| Caching {
                reach,
                positions: false,
                revision: "0".to_owned(),
                tools: Vec::new(),
            });
            Box::new(Murk { manifest })
        } else {
            Box::new(Ghost::new(self.0))
        };
        vec![
            Box::new(Word::new("fake/todo", "TODO")),
            Box::new(Word::new("fake/bad", "BAD")),
            ghost,
        ]
    }

    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        vec![
            Box::new(rewrite("fake/declare", "fn z", Safety::Safe, &[])),
            Box::new(rewrite("fake/declare-bad", "fn z BAD", Safety::Safe, &[])),
            Box::new(rewrite("fake/exorcise", "", Safety::Safe, &[])),
        ]
    }
}

/// Fixes the `TODO` of `a.toy` with `fixer`, which declares `z`; `ghost.toy` is
/// where the finding appears, and `calls` tells whether its function calls
/// `z`. With `repair`, the finding in `ghost.toy` has a fix.
fn haunted_by(
    (reach, fixer): (Option<Reach>, &str),
    ghost: &str,
    repair: bool,
) -> (tempfile::TempDir, FixReport) {
    ghosted((reach, fixer), ghost, (repair, false))
}

/// The same; `murky` says the ghost is a gap and not a finding.
fn ghosted(
    (reach, fixer): (Option<Reach>, &str),
    ghost: &str,
    (repair, murky): (bool, bool),
) -> (tempfile::TempDir, FixReport) {
    let dir = project("fn a\nTODO\n");
    fs::write(dir.path().join("ghost.toy"), ghost).unwrap();
    let mut registry = Registry::default();
    registry.register(&Haunted(reach, murky)).unwrap();
    let config = Config::parse_inline(
        "plugins = [\"fake\"]\n[rules]\n\"fake/todo\" = \"error\"\n\"fake/bad\" = \"error\"\n\"fake/ghost\" = \"error\"\n",
    )
    .unwrap();
    let engine = Engine::new(
        registry,
        config,
        lighthouse_test_support::bundled(),
        dir.path(),
    )
    .unwrap();
    let mut plan = plan(fixer, Safety::Safe);
    if repair {
        plan.insert(
            "fake/ghost",
            FixBinding {
                fixer: FixedBy::Fixer("fake/exorcise".to_owned()),
                cap: Safety::Safe,
                languages: Vec::new(),
                decision: FixDecision {
                    id: "fake/ghost".to_owned(),
                    requirement: String::new(),
                    context: String::new(),
                },
                needs: Needs::Nothing,
            },
        );
    }
    let report = engine.fix(&plan, &FixRun::default()).unwrap();
    (dir, report)
}

fn haunted(reach: Option<Reach>, calls: bool) -> (tempfile::TempDir, FixReport) {
    let ghost = if calls {
        "fn g\ncall z\nBOO\n"
    } else {
        "fn g\nBOO\n"
    };
    haunted_by((reach, "fake/declare"), ghost, false)
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
fn a_gap_that_turns_up_in_an_unchanged_file_charges_only_the_files_that_reach_it() {
    let far = ghosted(
        (Some(Reach::Neighbors), "fake/declare"),
        "fn g\nBOO\n",
        (false, true),
    );
    let near = ghosted(
        (Some(Reach::Neighbors), "fake/declare"),
        "fn g\ncall z\nBOO\n",
        (false, true),
    );

    assert!(is_kept(&far.0), "{:?}", far.1.notes);
    assert!(!is_kept(&near.0));
    assert!(near.1.notes[0].contains("murky"), "{:?}", near.1.notes);
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

#[test]
fn a_dependent_that_a_fix_breaks_is_repaired_by_the_safe_fix_of_its_own_finding() {
    let (dir, report) = haunted_by((Some(Reach::Global), "fake/declare"), "fn g\nBOO\n", true);

    assert!(is_kept(&dir), "{:?}", report.notes);
    assert_eq!(read(&dir, "ghost.toy"), "fn g\n\n");
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    let repair = report.applied.iter().find(|a| a.rule_id == "fake/ghost");
    assert!(repair.is_some_and(|a| a.follow_up), "{:?}", report.applied);
    // The first check, the change, and the safe fix that followed it.
    assert_eq!(checks_per_round(&report), [(1, 3)]);
}

#[test]
fn the_repair_of_a_dependent_is_undone_with_the_change_that_called_for_it() {
    let (dir, report) = haunted_by(
        (Some(Reach::Global), "fake/declare-bad"),
        "fn g\nBOO\n",
        true,
    );

    assert_eq!(text(&dir), "fn a\nTODO\n", "{:?}", report.notes);
    assert_eq!(read(&dir, "ghost.toy"), "fn g\nBOO\n");
    assert!(report.applied.is_empty(), "{:?}", report.applied);
    // The change and its follow-up are rolled back to the state the round began
    // from, whose check the round holds: no check of it again.
    assert_eq!(checks_per_round(&report), [(1, 3)]);
}

/// Replaces the `TODO` with `DONE`, but with `BAD` in `b.toy`.
struct Selective(FixerManifest);

impl Fixer for Selective {
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
            safety: Safety::Safe,
        })
    }
}

struct Unrelated;

impl Plugin for Unrelated {
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
        vec![Box::new(Selective(FixerManifest {
            id: "fake/selective".to_owned(),
            requires: Vec::new(),
        }))]
    }
}

#[test]
fn of_unrelated_sources_only_the_one_whose_fix_broke_its_own_file_is_rolled_back() {
    let dir = project("fn a\nTODO\n");
    for file in ["b.toy", "c.toy"] {
        fs::write(dir.path().join(file), "fn x\nTODO\n").unwrap();
    }
    let mut registry = Registry::default();
    registry.register(&Unrelated).unwrap();
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

    let report = engine
        .fix(&plan("fake/selective", Safety::Safe), &FixRun::default())
        .unwrap();

    assert_eq!(read(&dir, "a.toy"), "fn a\nDONE\n", "{:?}", report.notes);
    assert_eq!(read(&dir, "b.toy"), "fn x\nTODO\n");
    assert_eq!(read(&dir, "c.toy"), "fn x\nDONE\n");
    assert!(
        report
            .notes
            .iter()
            .all(|note| note.starts_with("rolled back b.toy")),
        "{:?}",
        report.notes
    );
}
