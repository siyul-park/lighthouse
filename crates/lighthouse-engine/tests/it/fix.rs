//! The fix orchestrator over a toy language: lines of `fn name` are symbols,
//! `TODO` and `BAD` are error findings, and `BROKEN` makes a file unanalyzable.
//! Fixers rewrite `TODO` into something, which lets each test choose whether
//! the fix is good, introduces an error, or breaks the file.

use std::{collections::BTreeSet, path::Path};

use lighthouse_engine::{Engine, Error, FixBinding, FixPlan, FixRun, FixedBy, Needs, unified_diff};
use lighthouse_model::{
    Safety,
    ucm::{EditOp, FixOutcome},
};
use lighthouse_plugin::{
    Error as PluginError, FixDecision, FixRequest, Fixer, FixerManifest, LanguageProvider, Plugin,
    PluginManifest, Registry, Rule,
};
use lighthouse_spec::Config;
use lighthouse_spec::{FixScope, Task};

use crate::support::*;

#[test]
fn a_fix_that_holds_is_applied_and_reported() {
    let dir = project("fn a\nTODO here\n");
    let engine = engine(&dir, "");

    let report = engine
        .fix(&plan("fake/to-done", Safety::Safe), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "fn a\nDONE here\n");
    assert_eq!(report.applied.len(), 1);
    assert_eq!(
        report.applied[0].fixer,
        FixedBy::Fixer("fake/to-done".to_owned())
    );
    assert_eq!(report.rounds, 1);
    assert!(report.declined.is_empty());
    assert!(report.finished.diagnostics.is_empty());
    assert_eq!(report.changes.len(), 1);
    assert!(
        unified_diff(
            Path::new("a.toy"),
            &report.changes[0].before,
            &report.changes[0].after
        )
        .contains("+DONE here")
    );
}

#[test]
fn a_fix_that_introduces_an_error_is_rolled_back() {
    let dir = project("fn a\nTODO here\n");
    let engine = engine(&dir, "");

    let report = engine
        .fix(&plan("fake/to-bad", Safety::Safe), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "fn a\nTODO here\n", "the file is as it was");
    assert!(report.applied.is_empty());
    assert!(report.changes.is_empty());
    let reason = &report.declined[0].reason;
    assert!(
        reason.contains("rolled back") && reason.contains("fake/bad"),
        "{reason}"
    );
    assert!(report.notes.iter().any(|n| n.contains("rolled back a.toy")));
}

#[test]
fn a_fix_that_makes_the_file_unanalyzable_is_rolled_back() {
    let dir = project("fn a\nTODO here\n");
    let engine = engine(&dir, "");

    let report = engine
        .fix(&plan("fake/to-broken", Safety::Safe), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "fn a\nTODO here\n");
    let reason = &report.declined[0].reason;
    assert!(reason.contains("no longer fully analyzed"), "{reason}");
}

#[test]
fn a_dry_run_leaves_the_files_and_still_reports_the_changes() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let run = FixRun {
        dry_run: true,
        ..FixRun::default()
    };

    let report = engine
        .fix(&plan("fake/to-done", Safety::Safe), &run)
        .unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert_eq!(report.applied.len(), 1);
    assert_eq!(report.changes[0].after, "DONE\n");
}

#[test]
fn an_incomplete_analysis_blocks_fixing() {
    let dir = project("BROKEN\nTODO\n");
    let engine = engine(&dir, "");

    let error = engine
        .fix(&plan("fake/to-done", Safety::Safe), &FixRun::default())
        .unwrap_err();

    assert!(matches!(error, Error::Fix(_)), "{error}");
    assert!(error.to_string().contains("incomplete"));
    assert_eq!(text(&dir), "BROKEN\nTODO\n");
}

#[test]
fn an_unsafe_fix_is_previewed_until_the_run_may_write_it() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let unsafe_fix = plan("fake/to-done", Safety::Unsafe);

    let held = engine.fix(&unsafe_fix, &FixRun::default()).unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert!(held.applied.is_empty() && held.declined.is_empty());
    let shown = &held.previews[0];
    assert_eq!(shown.rule_id, "fake/todo");
    assert_eq!(shown.fixer, FixedBy::Fixer("fake/to-done".to_owned()));
    assert_eq!(shown.preview.safety, Safety::Unsafe);
    assert!(shown.preview.files[0].hunks.contains("+DONE"));

    let run = FixRun {
        unsafe_fixes: Some(true),
        ..FixRun::default()
    };
    let written = engine.fix(&unsafe_fix, &run).unwrap();
    assert_eq!(text(&dir), "DONE\n");
    assert!(written.previews.is_empty());
}

#[test]
fn unsafe_in_the_project_writes_an_unsafe_fix_of_any_provider() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "[fix]\nunsafe = true\n");

    let report = engine
        .fix(&plan("fake/to-done", Safety::Unsafe), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "DONE\n");
    assert_eq!(report.applied[0].safety, Safety::Unsafe);
    assert!(report.previews.is_empty());
}

#[test]
fn no_unsafe_fixes_keeps_a_fix_a_project_would_write_as_a_preview() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "[fix]\nunsafe = true\n");
    let run = FixRun {
        unsafe_fixes: Some(false),
        ..FixRun::default()
    };

    let report = engine
        .fix(&plan("fake/to-done", Safety::Unsafe), &run)
        .unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert!(report.applied.is_empty());
    assert_eq!(report.previews.len(), 1);
}

#[test]
fn the_project_has_the_last_word_on_the_safety_of_a_fix() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let hardened =
        plan("fake/to-done", Safety::Safe).with_safety([("fake/todo".to_owned(), Safety::Unsafe)]);

    let held = engine.fix(&hardened, &FixRun::default()).unwrap();

    assert_eq!(text(&dir), "TODO\n", "a fix the project calls unsafe waits");
    assert_eq!(held.previews[0].preview.safety, Safety::Unsafe);

    let trusted =
        plan("fake/to-done", Safety::Unsafe).with_safety([("fake/todo".to_owned(), Safety::Safe)]);
    let written = engine.fix(&trusted, &FixRun::default()).unwrap();

    assert_eq!(
        text(&dir),
        "DONE\n",
        "a fix the project calls safe is written"
    );
    assert_eq!(written.applied[0].safety, Safety::Safe);
}

#[test]
fn a_dry_run_previews_what_it_may_not_write_and_writes_nothing() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let run = FixRun {
        dry_run: true,
        ..FixRun::default()
    };

    let report = engine
        .fix(&plan("fake/to-done", Safety::Unsafe), &run)
        .unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert_eq!(report.previews.len(), 1);
    assert!(report.changes.is_empty());
}

#[test]
fn the_decision_caps_what_a_fixer_claims() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    // The fixer claims `safe`; the decision says `unsafe`.
    let capped = plan("fake/to-done-claimed-safe", Safety::Unsafe);

    let report = engine.fix(&capped, &FixRun::default()).unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert_eq!(report.previews[0].preview.safety, Safety::Unsafe);
}

#[test]
fn a_missing_provider_capability_declines_instead_of_guessing() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");

    let report = engine
        .fix(&plan("fake/needs-extent", Safety::Safe), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert!(
        report.declined[0]
            .reason
            .contains("lacks the capability `extent`")
    );
}

#[test]
fn findings_in_one_file_are_fixed_together() {
    // Two TODOs: the proposals touch different ranges and both apply at once.
    let dir = project("TODO TODO\n");
    let engine = engine(&dir, "");

    let report = engine
        .fix(&plan("fake/to-done", Safety::Safe), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "DONE DONE\n");
    assert_eq!(report.applied.len(), 2);
}

#[test]
fn an_unknown_fixer_override_is_an_error() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let run = FixRun {
        fixer: Some("fake/nope".to_owned()),
        ..FixRun::default()
    };

    let error = engine.fix(&FixPlan::default(), &run).unwrap_err();

    assert!(error.to_string().contains("unknown fixer"));
}

#[test]
fn a_fixer_override_applies_to_a_rule_without_a_fix_as_an_unsafe_fix() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let run = FixRun {
        fixer: Some("fake/to-done".to_owned()),
        rules: vec!["fake/todo".to_owned()],
        ..FixRun::default()
    };

    let held = engine.fix(&FixPlan::default(), &run).unwrap();
    assert_eq!(text(&dir), "TODO\n");
    assert!(held.declined.is_empty());
    assert_eq!(held.previews[0].preview.safety, Safety::Unsafe);

    let run = FixRun {
        unsafe_fixes: Some(true),
        ..run
    };
    engine.fix(&FixPlan::default(), &run).unwrap();
    assert_eq!(text(&dir), "DONE\n");
}

#[test]
fn findings_a_run_skips_are_left_alone() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let found = engine.check(&[], &[]).unwrap();
    let run = FixRun {
        skip: found
            .diagnostics
            .iter()
            .map(|d| d.fingerprint.clone())
            .collect(),
        ..FixRun::default()
    };

    let report = engine
        .fix(&plan("fake/to-done", Safety::Safe), &run)
        .unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert!(report.applied.is_empty() && report.declined.is_empty());
}

#[test]
fn fixes_stop_after_a_bounded_number_of_rounds() {
    // A fixer that turns TODO into TODO TODO would never finish; the rounds
    // are bounded and the run says so.
    let dir = project("TODO\n");
    let mut registry = Registry::default();
    struct Grow(FixerManifest);
    impl Fixer for Grow {
        fn manifest(&self) -> &FixerManifest {
            &self.0
        }
        fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
            Ok(FixOutcome::Proposed {
                description: "grow".to_owned(),
                ops: vec![EditOp::Replace {
                    file: request.finding.file.clone(),
                    span: request.finding.span,
                    text: "x\nTODO".to_owned(),
                }],
                safety: Safety::Safe,
            })
        }
    }
    struct Growing(PluginManifest, FixerManifest);
    impl Plugin for Growing {
        fn manifest(&self) -> &PluginManifest {
            &self.0
        }
        fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
            vec![Box::new(Toy(toy()))]
        }
        fn rules(&self) -> Vec<Box<dyn Rule>> {
            vec![Box::new(Word::new("grow/todo", "TODO"))]
        }
        fn fixers(&self) -> Vec<Box<dyn Fixer>> {
            vec![Box::new(Grow(self.1.clone()))]
        }
    }
    let fixer = FixerManifest {
        id: "grow/grow".to_owned(),
        requires: Vec::new(),
    };
    registry
        .register(&Growing(
            PluginManifest {
                id: "grow".to_owned(),
                version: "0".to_owned(),
            },
            fixer,
        ))
        .unwrap();
    let config =
        Config::parse_inline("plugins = [\"grow\"]\n[rules]\n\"grow/todo\" = \"warn\"\n").unwrap();
    let engine = Engine::new(
        registry,
        config,
        lighthouse_test_support::bundled(),
        dir.path(),
    )
    .unwrap();
    let mut plan = FixPlan::default();
    plan.insert(
        "grow/todo",
        FixBinding {
            fixer: FixedBy::Fixer("grow/grow".to_owned()),
            cap: Safety::Safe,
            languages: Vec::new(),
            decision: FixDecision {
                id: "grow/todo".to_owned(),
                requirement: String::new(),
                context: String::new(),
            },
            needs: Needs::Nothing,
        },
    );

    let report = engine.fix(&plan, &FixRun::default()).unwrap();

    assert_eq!(report.rounds, lighthouse_engine::MAX_ROUNDS);
    assert!(report.notes.iter().any(|n| n.contains("stopped after")));
}

#[test]
fn findings_that_propose_the_same_edit_share_one_application() {
    let dir = project("TODO and TODO\n");
    let engine = engine(&dir, "");

    let report = engine
        .fix(&plan("fake/line", Safety::Safe), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "DONE and DONE\n");
    assert_eq!(report.applied.len(), 2, "both findings count as fixed");
    assert_eq!(report.rounds, 1);
}

#[test]
fn the_plan_of_a_catalog_binds_each_fixable_rule_to_the_fixer_of_its_pattern() {
    let catalog = lighthouse_test_support::bundled();

    let plan = FixPlan::from_catalog(catalog, lighthouse_test_support::bundled());

    let groups = plan.get("design/declaration-groups").unwrap();
    assert_eq!(
        groups.fixer,
        FixedBy::Fixer("design/declaration-groups".to_owned())
    );
    assert_eq!(groups.cap, Safety::Safe);
    let banners = plan.get("design/no-noise-comment").unwrap();
    assert_eq!(banners.cap, Safety::Unsafe);
    assert!(plan.get("design/exported-doc").is_none());
}

#[test]
fn model_fixes_are_given_only_to_the_listed_rules_that_have_a_check_and_no_fix_of_their_own() {
    let catalog = lighthouse_test_support::bundled();
    let listed: BTreeSet<String> = ["design/exported-doc", "design/declaration-groups"]
        .map(str::to_owned)
        .into();

    let plan = FixPlan::from_catalog(catalog, lighthouse_test_support::bundled()).with_model_fixes(
        catalog,
        &listed,
        FixScope::Files,
    );

    let documented = plan.get("design/exported-doc").unwrap();
    assert_eq!(documented.fixer, FixedBy::Model);
    assert_eq!(documented.cap, Safety::Unsafe);
    assert_eq!(
        documented.needs,
        Needs::Model {
            decision: "design/exported-doc".to_owned(),
            task: Task::Agent
        }
    );
    assert_eq!(documented.decision.id, "design/exported-doc");
    let groups = plan.get("design/declaration-groups").unwrap();
    assert_eq!(
        groups.fixer,
        FixedBy::Fixer("design/declaration-groups".to_owned()),
        "a fix of its own stays"
    );
    assert!(
        plan.get("design/no-redundant-qualifier").is_none(),
        "a rule nobody listed has no model fix"
    );
}

#[test]
fn a_fix_changes_a_declaration_and_its_callers_in_other_files_by_default() {
    let dir = callers();
    let engine = engine(&dir, "");

    let report = engine
        .fix(&plan("fake/callers", Safety::Safe), &FixRun::default())
        .unwrap();

    assert!(report.declined.is_empty(), "{:?}", report.declined);
    assert_eq!(read(&dir, "a.toy"), "fn a 2\nDONE here\n");
    assert_eq!(read(&dir, "b.toy"), "fn b\ncall a 2\n");
    assert_eq!(read(&dir, "c.toy"), "fn c\ncall a 2\n");
}

#[test]
fn a_caller_whose_own_declaration_changes_brings_its_callers_in() {
    let dir = callers();
    let engine = engine(&dir, "");

    let report = engine
        .fix(&plan("fake/transitive", Safety::Safe), &FixRun::default())
        .unwrap();

    assert!(report.declined.is_empty(), "{:?}", report.declined);
    assert_eq!(read(&dir, "b.toy"), "fn b 2\ncall a\n");
    assert_eq!(read(&dir, "e.toy"), "fn e\ncall b 2\n");
}

#[test]
fn a_file_that_refers_to_nothing_changed_is_declined_unless_the_scope_is_files() {
    let dir = callers();
    let engine = engine(&dir, "");

    let declined = engine
        .fix(&plan("fake/unrelated", Safety::Safe), &FixRun::default())
        .unwrap();

    assert!(
        declined.declined[0].reason.contains("`d.toy`"),
        "{:?}",
        declined.declined
    );
    assert_eq!(read(&dir, "d.toy"), "fn d\n");
    assert_eq!(read(&dir, "a.toy"), "fn a\nTODO here\n");

    let wide = plan("fake/unrelated", Safety::Safe).with_scope("fake/todo", FixScope::Files);
    let applied = engine.fix(&wide, &FixRun::default()).unwrap();

    assert!(applied.declined.is_empty(), "{:?}", applied.declined);
    assert_eq!(read(&dir, "d.toy"), "fn d 2\n");
}

#[test]
fn the_scope_file_declines_an_edit_of_a_caller() {
    let dir = callers();
    let engine = engine(&dir, "");
    let narrow = plan("fake/callers", Safety::Safe).with_scope("fake/todo", FixScope::File);

    let report = engine.fix(&narrow, &FixRun::default()).unwrap();

    assert!(
        report.declined[0].reason.contains("scope `file`"),
        "{:?}",
        report.declined
    );
    assert_eq!(read(&dir, "b.toy"), "fn b\ncall a\n");
}
