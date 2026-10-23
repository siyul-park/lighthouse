//! Fix previews over the toy language: what a fix run leaves for a hand.

use lighthouse_engine::{FixPlan, FixRun, Needs};
use lighthouse_model::Safety;

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
