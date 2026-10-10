//! Fix previews over the toy language: the proposal applied in memory.

use lighthouse_engine::FixPlan;
use lighthouse_model::Safety;
use lighthouse_spec::Catalog;

use crate::support::*;

#[test]
fn engine_preview_fixes_shows_the_change_and_writes_nothing() {
    let dir = project("fn a\nTODO here\n");
    let engine = engine(&dir, "");
    let outcome = engine.check(&[dir.path().to_path_buf()], &[]).unwrap();
    let findings: Vec<_> = outcome.diagnostics.iter().collect();

    let previews = engine.preview_fixes(
        &plan("fake/to-done", Safety::Safe, true),
        &outcome,
        &findings,
    );

    assert_eq!(previews.len(), 1);
    assert_eq!(previews[0].safety, Safety::Safe);
    assert_eq!(previews[0].changes[0].after, "fn a\nDONE here\n");
    assert_eq!(previews[0].edits.len(), 1);
    assert_eq!(text(&dir), "fn a\nTODO here\n", "nothing was written");
}

#[test]
fn engine_preview_fixes_calls_a_fix_suggested_when_applying_needs_unsafe_fixes() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let outcome = engine.check(&[dir.path().to_path_buf()], &[]).unwrap();
    let findings: Vec<_> = outcome.diagnostics.iter().collect();

    for plan in [
        plan("fake/to-done", Safety::Suggested, true),
        plan("fake/to-done", Safety::Safe, false),
    ] {
        let previews = engine.preview_fixes(&plan, &outcome, &findings);
        assert_eq!(previews[0].safety, Safety::Suggested);
    }
}

#[test]
fn engine_preview_fixes_skips_what_cannot_be_proposed() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let outcome = engine.check(&[dir.path().to_path_buf()], &[]).unwrap();
    let findings: Vec<_> = outcome.diagnostics.iter().collect();

    let none = engine.preview_fixes(&FixPlan::default(), &outcome, &findings);
    assert!(none.is_empty(), "a rule without a fix");
    let unknown = engine.preview_fixes(
        &plan("fake/missing", Safety::Safe, true),
        &outcome,
        &findings,
    );
    assert!(unknown.is_empty(), "a fixer that is not registered");
}

#[test]
fn fix_plan_runs_command_only_for_fixers_that_run_a_program() {
    let plan = FixPlan::from_catalog(Catalog::bundled());
    assert!(!plan.runs_command("design/declaration-groups"));
    assert!(!plan.runs_command("nope/nope"));
}
