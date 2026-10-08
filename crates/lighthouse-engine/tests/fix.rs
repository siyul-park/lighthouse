//! The fix orchestrator over a toy language: lines of `fn name` are symbols,
//! `TODO` and `BAD` are error findings, and `BROKEN` makes a file unanalyzable.
//! Fixers rewrite `TODO` into something, which lets each test choose whether
//! the fix is good, introduces an error, or breaks the file.

use std::path::Path;

use lighthouse_config::Config;
use lighthouse_engine::{Engine, Error, FixBinding, FixPlan, FixRun, unified_diff};
use lighthouse_model::{EditOp, FixOutcome, Safety};
use lighthouse_plugin::{
    Error as PluginError, FixDecision, FixRequest, Fixer, FixerManifest, LanguageProvider, Plugin,
    PluginManifest, Registry, Rule,
};

mod support;
use support::*;

#[test]
fn a_fix_that_holds_is_applied_and_reported() {
    let dir = project("fn a\nTODO here\n");
    let engine = engine(&dir, "");

    let report = engine
        .fix(
            &plan("fake/to-done", Safety::Safe, true),
            &FixRun::default(),
        )
        .unwrap();

    assert_eq!(text(&dir), "fn a\nDONE here\n");
    assert_eq!(report.applied.len(), 1);
    assert_eq!(report.applied[0].fixer, "fake/to-done");
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
        .fix(&plan("fake/to-bad", Safety::Safe, true), &FixRun::default())
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
        .fix(
            &plan("fake/to-broken", Safety::Safe, true),
            &FixRun::default(),
        )
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
        .fix(&plan("fake/to-done", Safety::Safe, true), &run)
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
        .fix(
            &plan("fake/to-done", Safety::Safe, true),
            &FixRun::default(),
        )
        .unwrap_err();

    assert!(matches!(error, Error::Fix(_)), "{error}");
    assert!(error.to_string().contains("incomplete"));
    assert_eq!(text(&dir), "BROKEN\nTODO\n");
}

#[test]
fn a_suggested_fix_needs_unsafe_fixes() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let suggested = plan("fake/to-done", Safety::Suggested, true);

    let held = engine.fix(&suggested, &FixRun::default()).unwrap();
    assert_eq!(text(&dir), "TODO\n");
    assert!(held.declined[0].reason.contains("--unsafe-fixes"));

    let run = FixRun {
        unsafe_fixes: true,
        ..FixRun::default()
    };
    engine.fix(&suggested, &run).unwrap();
    assert_eq!(text(&dir), "DONE\n");
}

#[test]
fn the_decision_caps_what_a_fixer_claims() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    // The fixer claims `safe`; the decision says `suggested`.
    let capped = plan("fake/to-done-claimed-safe", Safety::Suggested, true);

    let report = engine.fix(&capped, &FixRun::default()).unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert!(report.declined[0].reason.contains("suggested"));
}

#[test]
fn a_non_mechanical_rule_is_not_fixed_without_unsafe_fixes() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");

    engine
        .fix(
            &plan("fake/to-done", Safety::Safe, false),
            &FixRun::default(),
        )
        .unwrap();

    assert_eq!(text(&dir), "TODO\n");
}

#[test]
fn a_missing_provider_capability_declines_instead_of_guessing() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");

    let report = engine
        .fix(
            &plan("fake/needs-extent", Safety::Safe, true),
            &FixRun::default(),
        )
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
        .fix(
            &plan("fake/to-done", Safety::Safe, true),
            &FixRun::default(),
        )
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
fn a_fixer_override_applies_to_a_rule_without_a_fix_as_a_suggestion() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let run = FixRun {
        fixer: Some("fake/to-done".to_owned()),
        rules: vec!["fake/todo".to_owned()],
        ..FixRun::default()
    };

    let held = engine.fix(&FixPlan::default(), &run).unwrap();
    assert_eq!(text(&dir), "TODO\n");
    assert!(held.declined[0].reason.contains("--unsafe-fixes"));

    let run = FixRun {
        unsafe_fixes: true,
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
        .fix(&plan("fake/to-done", Safety::Safe, true), &run)
        .unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert!(report.applied.is_empty() && report.declined.is_empty());
}

#[test]
fn the_formatter_runs_on_a_changed_file() {
    let dir = project("TODO   here\n");
    let formatter = r#"
[languages.toy]
formatter = ["sh", "-c", "tr -s ' ' < \"$0\" > \"$0.tmp\" && mv \"$0.tmp\" \"$0\""]
"#;
    let engine = engine(&dir, formatter);

    engine
        .fix(&plan("fake/to-done", Safety::Safe, true), &trusted())
        .unwrap();

    assert_eq!(text(&dir), "DONE here\n");
}

#[test]
fn a_formatter_that_fails_rolls_the_file_back() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "[languages.toy]\nformatter = [\"false\"]\n");

    let report = engine
        .fix(&plan("fake/to-done", Safety::Safe, true), &trusted())
        .unwrap();

    assert_eq!(text(&dir), "TODO\n");
    assert!(report.declined[0].reason.contains("formatter"));
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
    let engine = Engine::new(registry, config, dir.path()).unwrap();
    let mut plan = FixPlan::default();
    plan.insert(
        "grow/todo",
        FixBinding {
            fixer: "grow/grow".to_owned(),
            cap: Safety::Safe,
            mechanical: true,
            decision: FixDecision {
                id: "grow/todo".to_owned(),
                requirement: String::new(),
                intent: String::new(),
            },
            unsupported: None,
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
        .fix(&plan("fake/line", Safety::Safe, true), &FixRun::default())
        .unwrap();

    assert_eq!(text(&dir), "DONE and DONE\n");
    assert_eq!(report.applied.len(), 2, "both findings count as fixed");
    assert_eq!(report.rounds, 1);
}

#[test]
fn the_plan_of_a_catalog_binds_each_fixable_rule_to_the_fixer_of_its_pattern() {
    let catalog = lighthouse_spec::Catalog::bundled();

    let plan = FixPlan::from_catalog(catalog);

    let groups = plan.get("design/declaration-groups").unwrap();
    assert_eq!(groups.fixer, "design/declaration-groups");
    assert_eq!(groups.cap, Safety::Safe);
    assert!(groups.mechanical);
    let banners = plan.get("design/section-banners").unwrap();
    assert_eq!(banners.cap, Safety::Suggested);
    assert!(!banners.mechanical);
    assert!(plan.get("design/exported-doc").is_none());
}
