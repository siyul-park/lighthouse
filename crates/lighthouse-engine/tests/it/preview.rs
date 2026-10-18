//! Fix previews over the toy language: the proposal applied in memory.

use lighthouse_engine::{FixPlan, Needs};
use lighthouse_model::Safety;
use lighthouse_spec::Catalog;

use crate::support::*;

#[test]
fn engine_preview_fixes_shows_the_change_and_writes_nothing() {
    let dir = project("fn a\nTODO here\n");
    let engine = engine(&dir, "");
    let outcome = engine.check(&[dir.path().to_path_buf()], &[]).unwrap();
    let findings: Vec<_> = outcome.diagnostics.iter().collect();

    let previews = engine.preview_fixes(&plan("fake/to-done", Safety::Safe), &findings);

    assert_eq!(previews.len(), 1);
    assert_eq!(previews[0].safety, Safety::Safe);
    assert!(previews[0].files[0].hunks.contains("+DONE here"));
    assert_eq!(previews[0].files[0].edits.len(), 1);
    assert_eq!(text(&dir), "fn a\nTODO here\n", "nothing was written");
}

#[test]
fn engine_preview_fixes_calls_a_fix_unsafe_when_writing_it_needs_permission() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let outcome = engine.check(&[dir.path().to_path_buf()], &[]).unwrap();
    let findings: Vec<_> = outcome.diagnostics.iter().collect();

    let previews = engine.preview_fixes(&plan("fake/to-done", Safety::Unsafe), &findings);

    assert_eq!(previews[0].safety, Safety::Unsafe);
}

#[test]
fn engine_preview_fixes_skips_what_cannot_be_proposed() {
    let dir = project("TODO\n");
    let engine = engine(&dir, "");
    let outcome = engine.check(&[dir.path().to_path_buf()], &[]).unwrap();
    let findings: Vec<_> = outcome.diagnostics.iter().collect();

    let none = engine.preview_fixes(&FixPlan::default(), &findings);
    assert!(none.is_empty(), "a rule without a fix");
    let unknown = engine.preview_fixes(&plan("fake/missing", Safety::Safe), &findings);
    assert!(unknown.is_empty(), "a fixer that is not registered");
}

#[test]
fn fix_plan_needs_a_program_only_for_fixers_that_run_one() {
    let plan = FixPlan::from_catalog(Catalog::bundled());
    let needs = |rule: &str| plan.get(rule).map(|binding| binding.needs.clone());
    assert_eq!(needs("design/declaration-groups"), Some(Needs::Nothing));
    assert_eq!(needs("nope/nope"), None);
}
