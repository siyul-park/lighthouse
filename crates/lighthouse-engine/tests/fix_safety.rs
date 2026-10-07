//! The orchestrator's guarantees: verification in memory, a write at the end only,
//! trust, eligibility, containment and the stop conditions.

use std::fs;

use lighthouse_config::Config;
use lighthouse_engine::{Engine, FixBinding, FixPlan, FixRun};
use lighthouse_model::{Diagnostic, EditOp, FixOutcome, Options, Safety};
use lighthouse_plugin::{
    Ctx, Error as PluginError, FixPattern, FixRequest, Fixer, FixerManifest, LanguageProvider,
    Plugin, PluginManifest, Registry, Rule, RuleManifest,
};

mod support;
use support::*;

#[test]
fn an_untrusted_project_is_not_formatted_but_is_still_verified() {
    let dir = project("TODO   here\n");
    let formatter = "[languages.toy]\nformatter = [\"false\"]\n";
    let unformatted = engine(&dir, formatter);

    let report = unformatted
        .fix(
            &plan("fake/to-done", Safety::Safe, true),
            &FixRun::default(),
        )
        .unwrap();

    assert_eq!(text(&dir), "DONE   here\n", "applied, unformatted");
    assert!(
        report.notes.iter().any(|n| n.contains("not formatted")),
        "{:?}",
        report.notes
    );
    let bad = project("TODO\n");
    let verified = engine(&bad, "")
        .fix(&plan("fake/to-bad", Safety::Safe, true), &FixRun::default())
        .unwrap();
    assert_eq!(
        text(&bad),
        "TODO\n",
        "a fix that breaks things is still dropped"
    );
    assert!(verified.applied.is_empty());
}

#[test]
fn a_dry_run_verifies_in_memory_and_leaves_the_disk_byte_identical() {
    let dir = project("fn a\nTODO here\n");
    let path = dir.path().join("a.toy");
    let before = fs::read(&path).unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let engine = engine(&dir, "");
    let run = FixRun {
        dry_run: true,
        ..FixRun::default()
    };

    let report = engine
        .fix(&plan("fake/to-done", Safety::Safe, true), &run)
        .unwrap();

    assert_eq!(report.changes[0].after, "fn a\nDONE here\n");
    assert!(
        report.finished.diagnostics.is_empty(),
        "verified on the overlay"
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
    let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "no temporary files");
}

#[test]
fn a_file_that_changed_since_it_was_read_is_skipped_and_its_fix_is_declined() {
    let dir = project("TODO\n");
    let path = dir.path().join("a.toy");
    // A fixer that edits the file behind the orchestrator's back while it
    // proposes, as an editor saving would.
    struct Meddle(FixerManifest, std::path::PathBuf);
    impl Fixer for Meddle {
        fn manifest(&self) -> &FixerManifest {
            &self.0
        }
        fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
            fs::write(&self.1, "TODO\n// edited elsewhere\n").unwrap();
            Ok(FixOutcome::Proposed {
                description: "meddle".to_owned(),
                ops: vec![EditOp::Replace {
                    file: request.finding.file.clone(),
                    span: request.finding.span,
                    text: "DONE".to_owned(),
                }],
                safety: Safety::Safe,
            })
        }
    }
    struct Meddling(PluginManifest, FixerManifest, std::path::PathBuf);
    impl Plugin for Meddling {
        fn manifest(&self) -> &PluginManifest {
            &self.0
        }
        fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
            vec![Box::new(Toy(toy()))]
        }
        fn rules(&self) -> Vec<Box<dyn Rule>> {
            vec![Box::new(Word::new("meddle/todo", "TODO"))]
        }
        fn fixers(&self) -> Vec<Box<dyn Fixer>> {
            vec![Box::new(Meddle(self.1.clone(), self.2.clone()))]
        }
    }
    let mut registry = Registry::default();
    registry
        .register(&Meddling(
            PluginManifest {
                id: "meddle".to_owned(),
                version: "0".to_owned(),
            },
            FixerManifest {
                id: "meddle/meddle".to_owned(),
                requires: Vec::new(),
            },
            path.clone(),
        ))
        .unwrap();
    let config =
        Config::parse("plugins = [\"meddle\"]\n[rules]\n\"meddle/todo\" = \"error\"\n").unwrap();
    let engine = Engine::new(registry, config, dir.path()).unwrap();
    let mut plan = FixPlan::default();
    plan.insert(
        "meddle/todo",
        FixBinding {
            fixer: "meddle/meddle".to_owned(),
            cap: Safety::Safe,
            mechanical: true,
            pattern: FixPattern {
                id: "meddle/todo".to_owned(),
                requirement: String::new(),
                intent: String::new(),
            },
            unsupported: None,
        },
    );

    let report = engine.fix(&plan, &FixRun::default()).unwrap();

    assert_eq!(
        text(&dir),
        "TODO\n// edited elsewhere\n",
        "the other edit survives"
    );
    assert!(report.applied.is_empty());
    assert!(report.changes.is_empty());
    assert!(
        report.declined[0].reason.contains("changed concurrently"),
        "{:?}",
        report.declined
    );
}

#[test]
fn a_file_that_changes_while_verifying_is_not_written_over() {
    let dir = project("TODO\n");
    let path = dir.path().join("a.toy");
    // The formatter of a trusted project runs mid-run and, as an editor
    // saving would, changes the real file while the candidate is checked.
    let formatter = format!(
        "[languages.toy]\nformatter = [\"sh\", \"-c\", \"echo '// saved elsewhere' >> '{}'\"]\n",
        path.display()
    );
    let engine = engine(&dir, &formatter);

    let report = engine
        .fix(&plan("fake/to-done", Safety::Safe, true), &trusted())
        .unwrap();

    assert_eq!(
        text(&dir),
        "TODO\n// saved elsewhere\n",
        "the newer file wins"
    );
    assert!(report.applied.is_empty() && report.changes.is_empty());
    assert!(
        report.declined[0].reason.contains("changed concurrently"),
        "{:?}",
        report.declined
    );
    assert!(
        report.notes.iter().any(|n| n.contains("was not written")),
        "{:?}",
        report.notes
    );
}

#[test]
fn a_named_fixer_needs_the_rules_or_fingerprints_it_is_for() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let run = FixRun {
        fixer: Some("fake/to-done".to_owned()),
        ..FixRun::default()
    };

    let error = engine.fix(&FixPlan::default(), &run).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("needs the rules or fingerprints"),
        "{error}"
    );
}

#[test]
fn eligibility_is_decided_before_a_fixer_is_asked() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ASKED: AtomicUsize = AtomicUsize::new(0);
    struct Counting(FixerManifest);
    impl Fixer for Counting {
        fn manifest(&self) -> &FixerManifest {
            &self.0
        }
        fn fix(&self, _: &FixRequest) -> Result<FixOutcome, PluginError> {
            ASKED.fetch_add(1, Ordering::SeqCst);
            Ok(FixOutcome::declined("asked"))
        }
    }
    struct Counted(PluginManifest, FixerManifest);
    impl Plugin for Counted {
        fn manifest(&self) -> &PluginManifest {
            &self.0
        }
        fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
            vec![Box::new(Toy(toy()))]
        }
        fn rules(&self) -> Vec<Box<dyn Rule>> {
            vec![Box::new(Word::new("count/todo", "TODO"))]
        }
        fn fixers(&self) -> Vec<Box<dyn Fixer>> {
            vec![Box::new(Counting(self.1.clone()))]
        }
    }
    let dir = project("TODO\n");
    let mut registry = Registry::default();
    registry
        .register(&Counted(
            PluginManifest {
                id: "count".to_owned(),
                version: "0".to_owned(),
            },
            FixerManifest {
                id: "count/count".to_owned(),
                requires: Vec::new(),
            },
        ))
        .unwrap();
    let config =
        Config::parse("plugins = [\"count\"]\n[rules]\n\"count/todo\" = \"error\"\n").unwrap();
    let engine = Engine::new(registry, config, dir.path()).unwrap();
    let mut plan = FixPlan::default();
    plan.insert(
        "count/todo",
        FixBinding {
            fixer: "count/count".to_owned(),
            cap: Safety::Suggested,
            mechanical: true,
            pattern: FixPattern {
                id: "count/todo".to_owned(),
                requirement: String::new(),
                intent: String::new(),
            },
            unsupported: None,
        },
    );

    engine.fix(&plan, &FixRun::default()).unwrap();
    assert_eq!(
        ASKED.load(Ordering::SeqCst),
        0,
        "a suggested fix is not even asked for"
    );

    let run = FixRun {
        unsafe_fixes: true,
        ..FixRun::default()
    };
    engine.fix(&plan, &run).unwrap();
    assert_eq!(ASKED.load(Ordering::SeqCst), 1);
}

#[test]
fn an_unsupported_fix_declines_its_findings_with_its_reason() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let mut unsupported = plan("fake/to-done", Safety::Safe, true);
    unsupported.insert(
        "fake/todo",
        FixBinding {
            unsupported: Some(
                "the fix of this rule is of kind `rpc`, which is not yet supported".to_owned(),
            ),
            ..unsupported.get("fake/todo").unwrap().clone()
        },
    );

    let report = engine.fix(&unsupported, &FixRun::default()).unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert!(report.declined[0].reason.contains("rpc"));
}

#[test]
fn a_proposal_whose_edits_overlap_each_other_is_declined() {
    struct Twice(FixerManifest);
    impl Fixer for Twice {
        fn manifest(&self) -> &FixerManifest {
            &self.0
        }
        fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
            let edit = |text: &str| EditOp::Replace {
                file: request.finding.file.clone(),
                span: request.finding.span,
                text: text.to_owned(),
            };
            Ok(FixOutcome::Proposed {
                description: "twice".to_owned(),
                ops: vec![edit("A"), edit("B")],
                safety: Safety::Safe,
            })
        }
    }
    struct Twin(PluginManifest, FixerManifest);
    impl Plugin for Twin {
        fn manifest(&self) -> &PluginManifest {
            &self.0
        }
        fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
            vec![Box::new(Toy(toy()))]
        }
        fn rules(&self) -> Vec<Box<dyn Rule>> {
            vec![Box::new(Word::new("twin/todo", "TODO"))]
        }
        fn fixers(&self) -> Vec<Box<dyn Fixer>> {
            vec![Box::new(Twice(self.1.clone()))]
        }
    }
    let dir = project("TODO\n");
    let mut registry = Registry::default();
    registry
        .register(&Twin(
            PluginManifest {
                id: "twin".to_owned(),
                version: "0".to_owned(),
            },
            FixerManifest {
                id: "twin/twin".to_owned(),
                requires: Vec::new(),
            },
        ))
        .unwrap();
    let config =
        Config::parse("plugins = [\"twin\"]\n[rules]\n\"twin/todo\" = \"error\"\n").unwrap();
    let engine = Engine::new(registry, config, dir.path()).unwrap();
    let mut plan = FixPlan::default();
    plan.insert(
        "twin/todo",
        FixBinding {
            fixer: "twin/twin".to_owned(),
            cap: Safety::Safe,
            mechanical: true,
            pattern: FixPattern {
                id: "twin/todo".to_owned(),
                requirement: String::new(),
                intent: String::new(),
            },
            unsupported: None,
        },
    );

    let report = engine.fix(&plan, &FixRun::default()).unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert!(
        report.declined[0].reason.contains("overlap each other"),
        "{:?}",
        report.declined
    );
}

#[test]
fn fixes_that_undo_each_other_stop_the_run_and_name_their_rules() {
    // One rule flags TODO and BAD alike and its fixer turns each into the
    // other: no round makes anything worse, and the texts repeat.
    struct Either(RuleManifest);
    impl Rule for Either {
        fn manifest(&self) -> &RuleManifest {
            &self.0
        }
        fn validate(&self, _: &Options) -> Result<(), PluginError> {
            Ok(())
        }
        fn check(&self, ctx: &Ctx, options: &Options) -> Result<Vec<Diagnostic>, PluginError> {
            let mut found = Word::new("flip/any", "TODO").check(ctx, options)?;
            found.extend(Word::new("flip/any", "BAD").check(ctx, options)?);
            Ok(found)
        }
    }
    struct Flip(FixerManifest);
    impl Fixer for Flip {
        fn manifest(&self) -> &FixerManifest {
            &self.0
        }
        fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
            let to = if request.text.contains("TODO") {
                "BAD"
            } else {
                "TODO"
            };
            Ok(FixOutcome::Proposed {
                description: format!("flip to {to}"),
                ops: vec![EditOp::Replace {
                    file: request.finding.file.clone(),
                    span: request.finding.span,
                    text: to.to_owned(),
                }],
                safety: Safety::Safe,
            })
        }
    }
    struct Flipping(PluginManifest);
    impl Plugin for Flipping {
        fn manifest(&self) -> &PluginManifest {
            &self.0
        }
        fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
            vec![Box::new(Toy(toy()))]
        }
        fn rules(&self) -> Vec<Box<dyn Rule>> {
            vec![Box::new(Either(
                Word::new("flip/any", "TODO").manifest().clone(),
            ))]
        }
        fn fixers(&self) -> Vec<Box<dyn Fixer>> {
            vec![Box::new(Flip(FixerManifest {
                id: "flip/any".to_owned(),
                requires: Vec::new(),
            }))]
        }
    }
    let dir = project("TODO\n");
    let mut registry = Registry::default();
    registry
        .register(&Flipping(PluginManifest {
            id: "flip".to_owned(),
            version: "0".to_owned(),
        }))
        .unwrap();
    let config = Config::parse("plugins = [\"flip\"]\n[rules]\n\"flip/any\" = \"warn\"\n").unwrap();
    let engine = Engine::new(registry, config, dir.path()).unwrap();
    let mut plan = FixPlan::default();
    plan.insert(
        "flip/any",
        FixBinding {
            fixer: "flip/any".to_owned(),
            cap: Safety::Safe,
            mechanical: true,
            pattern: FixPattern {
                id: "flip/any".to_owned(),
                requirement: String::new(),
                intent: String::new(),
            },
            unsupported: None,
        },
    );

    let report = engine.fix(&plan, &FixRun::default()).unwrap();

    assert!(
        report.rounds < lighthouse_engine::MAX_ROUNDS,
        "{}",
        report.rounds
    );
    let note = report
        .notes
        .iter()
        .find(|n| n.contains("undo each other"))
        .expect("a note");
    assert!(note.contains("flip/any"), "{note}");
}

#[test]
fn a_fix_that_adds_a_warning_is_rolled_back_like_one_that_adds_an_error() {
    // `fake/to-warn` turns TODO into WARN, flagged by a warn-level rule.
    struct Warned(PluginManifest);
    impl Plugin for Warned {
        fn manifest(&self) -> &PluginManifest {
            &self.0
        }
        fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
            vec![Box::new(Toy(toy()))]
        }
        fn rules(&self) -> Vec<Box<dyn Rule>> {
            vec![
                Box::new(Word::new("warned/todo", "TODO")),
                Box::new(Word::new("warned/warn", "WARN")),
            ]
        }
        fn fixers(&self) -> Vec<Box<dyn Fixer>> {
            vec![Box::new(rewrite("warned/todo", "WARN", Safety::Safe, &[]))]
        }
    }
    let dir = project("TODO\n");
    let mut registry = Registry::default();
    registry
        .register(&Warned(PluginManifest {
            id: "warned".to_owned(),
            version: "0".to_owned(),
        }))
        .unwrap();
    let config = Config::parse(
        "plugins = [\"warned\"]\n[rules]\n\"warned/todo\" = \"error\"\n\"warned/warn\" = \"warn\"\n",
    )
    .unwrap();
    let engine = Engine::new(registry, config, dir.path()).unwrap();
    let mut plan = FixPlan::default();
    plan.insert(
        "warned/todo",
        FixBinding {
            fixer: "warned/todo".to_owned(),
            cap: Safety::Safe,
            mechanical: true,
            pattern: FixPattern {
                id: "warned/todo".to_owned(),
                requirement: String::new(),
                intent: String::new(),
            },
            unsupported: None,
        },
    );

    let report = engine.fix(&plan, &FixRun::default()).unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert!(
        report.declined[0].reason.contains("warned/warn"),
        "{:?}",
        report.declined
    );
}

#[cfg(unix)]
#[test]
fn a_written_file_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = project("TODO\n");
    let path = dir.path().join("a.toy");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();
    let engine = engine(&dir, "");

    engine
        .fix(
            &plan("fake/to-done", Safety::Safe, true),
            &FixRun::default(),
        )
        .unwrap();

    assert_eq!(text(&dir), "DONE\n");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o751
    );
}

#[test]
fn a_fix_cannot_reach_outside_the_project_or_into_a_symlink() {
    struct Reach(FixerManifest, &'static str);
    impl Fixer for Reach {
        fn manifest(&self) -> &FixerManifest {
            &self.0
        }
        fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
            Ok(FixOutcome::Proposed {
                description: "reach".to_owned(),
                ops: vec![EditOp::Replace {
                    file: self.1.into(),
                    span: request.finding.span,
                    text: "pwned".to_owned(),
                }],
                safety: Safety::Safe,
            })
        }
    }
    struct Reaching(PluginManifest, &'static str);
    impl Plugin for Reaching {
        fn manifest(&self) -> &PluginManifest {
            &self.0
        }
        fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
            vec![Box::new(Toy(toy()))]
        }
        fn rules(&self) -> Vec<Box<dyn Rule>> {
            vec![Box::new(Word::new("reach/todo", "TODO"))]
        }
        fn fixers(&self) -> Vec<Box<dyn Fixer>> {
            vec![Box::new(Reach(
                FixerManifest {
                    id: "reach/reach".to_owned(),
                    requires: Vec::new(),
                },
                self.1,
            ))]
        }
    }
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("victim.toy"), "TODO\n").unwrap();
    let dir = project("TODO\n");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), dir.path().join("out")).unwrap();
    let targets: &[(&'static str, &str)] = &[
        ("../victim.toy", "not a relative, normalized"),
        ("/etc/hosts", "not a relative, normalized"),
        ("missing.toy", "not a file of the project"),
        #[cfg(unix)]
        ("out/victim.toy", "not a file of the project"),
    ];
    for (target, why) in targets {
        let mut registry = Registry::default();
        registry
            .register(&Reaching(
                PluginManifest {
                    id: "reach".to_owned(),
                    version: "0".to_owned(),
                },
                target,
            ))
            .unwrap();
        let config =
            Config::parse("plugins = [\"reach\"]\n[rules]\n\"reach/todo\" = \"error\"\n").unwrap();
        let engine = Engine::new(registry, config, dir.path()).unwrap();
        let mut plan = FixPlan::default();
        plan.insert(
            "reach/todo",
            FixBinding {
                fixer: "reach/reach".to_owned(),
                cap: Safety::Safe,
                mechanical: true,
                pattern: FixPattern {
                    id: "reach/todo".to_owned(),
                    requirement: String::new(),
                    intent: String::new(),
                },
                unsupported: None,
            },
        );

        let report = engine.fix(&plan, &FixRun::default()).unwrap();

        let reason = &report.declined[0].reason;
        assert!(reason.contains(why), "{target}: {reason}");
        assert_eq!(text(&dir), "TODO\n");
        assert_eq!(
            fs::read_to_string(outside.path().join("victim.toy")).unwrap(),
            "TODO\n"
        );
    }
}
