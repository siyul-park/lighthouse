//! The previews of a run: complete over the state the run ends in, kept with
//! the files they were verified over, and applied only as they were shown.

use std::fs;

use lighthouse_engine::{Engine, FixPlan, FixRun, Run, cache};
use lighthouse_model::Safety;
use tempfile::TempDir;

use crate::support::*;

/// `TODO` and `BAD` are errors. The fix of `TODO` is safe but makes a `BAD`
/// more, so the checks roll it back; the fix of `BAD` is unsafe.
fn stuck() -> FixPlan {
    let mut both = plan("fake/to-bad", Safety::Safe);
    let unsafe_plan = plan("fake/to-done", Safety::Unsafe);
    let bad = unsafe_plan.get("fake/todo").unwrap().clone();
    both.insert(
        "fake/bad",
        lighthouse_engine::FixBinding {
            decision: lighthouse_plugin::FixDecision {
                id: "fake/bad".to_owned(),
                requirement: String::new(),
                context: String::new(),
            },
            ..bad
        },
    );
    both
}

/// The engine of `dir`, keeping its previews in a cache of its own.
fn keeping(dir: &TempDir, cache_dir: &TempDir) -> Engine {
    engine(dir, "").with_cache(cache_dir.path().to_path_buf(), cache::DEFAULT_LIMIT)
}

fn fingerprints(run: &Run, rule: &str) -> Vec<String> {
    run.diagnostics
        .iter()
        .filter(|d| d.rule_id == rule)
        .map(|d| d.fingerprint.as_str().to_owned())
        .collect()
}

#[test]
fn a_run_that_stops_early_still_previews_what_it_withheld() {
    let dir = project("TODO\nBAD\n");
    let engine = engine(&dir, "");

    let report = engine.fix(&stuck(), &FixRun::default()).unwrap();

    assert_eq!(text(&dir), "TODO\nBAD\n", "the safe fix was rolled back");
    let previewed: Vec<_> = report.previews.iter().map(|p| p.rule_id.as_str()).collect();
    assert_eq!(previewed, ["fake/bad"], "{:?}", report.declined);
}

#[test]
fn a_preview_names_the_files_it_was_verified_over_and_its_batch() {
    let dir = project("BAD\n");
    let engine = engine(&dir, "");

    let report = engine.fix(&stuck(), &FixRun::default()).unwrap();

    let preview = &report.previews[0];
    assert_eq!(
        preview.batch,
        std::slice::from_ref(&preview.preview.fingerprint)
    );
    assert_eq!(preview.bases.len(), 1);
    assert_eq!(preview.bases.keys().next().unwrap().to_str(), Some("a.toy"));
}

#[test]
fn a_kept_preview_is_applied_as_it_was_shown() {
    let dir = project("BAD\n");
    let cache_dir = tempfile::tempdir().unwrap();
    let engine = keeping(&dir, &cache_dir);
    let shown = engine.fix(&stuck(), &FixRun::default()).unwrap();
    assert_eq!(text(&dir), "BAD\n");
    let fingerprint = shown.previews[0].preview.fingerprint.as_str().to_owned();

    let applied = engine
        .fix(
            &stuck(),
            &FixRun {
                fingerprints: vec![fingerprint],
                unsafe_fixes: true,
                ..FixRun::default()
            },
        )
        .unwrap();

    assert_eq!(applied.applied.len(), 1, "{:?}", applied.declined);
    assert_eq!(text(&dir), "DONE\n");
}

#[test]
fn a_changed_base_is_a_conflict_for_a_deterministic_fix_too() {
    let dir = project("BAD\n");
    let cache_dir = tempfile::tempdir().unwrap();
    let engine = keeping(&dir, &cache_dir);
    let shown = engine.fix(&stuck(), &FixRun::default()).unwrap();
    let fingerprint = shown.previews[0].preview.fingerprint.as_str().to_owned();
    fs::write(dir.path().join("a.toy"), "BAD\nmore\n").unwrap();

    let applied = engine
        .fix(
            &stuck(),
            &FixRun {
                fingerprints: vec![fingerprint],
                unsafe_fixes: true,
                ..FixRun::default()
            },
        )
        .unwrap();

    assert!(applied.applied.is_empty());
    assert_eq!(text(&dir), "BAD\nmore\n", "nothing was written");
    let reason = &applied.declined[0].reason;
    assert!(
        reason.contains("not as it was when the preview was made"),
        "{reason}"
    );
}

#[test]
fn a_preview_that_is_not_kept_is_declined_not_made_again() {
    let dir = project("BAD\n");
    let engine = engine(&dir, "");
    let fingerprint = {
        let run = engine.check(&[], &[]).unwrap();
        fingerprints(&run, "fake/bad")[0].clone()
    };

    let applied = engine
        .fix(
            &stuck(),
            &FixRun {
                fingerprints: vec![fingerprint],
                unsafe_fixes: true,
                ..FixRun::default()
            },
        )
        .unwrap();

    assert!(applied.applied.is_empty());
    assert_eq!(text(&dir), "BAD\n");
    let reason = &applied.declined[0].reason;
    assert!(reason.contains("the preview is not kept"), "{reason}");
}

#[test]
fn the_preview_of_a_check_is_the_preview_of_a_fix_run() {
    let dir = project("BAD\n");
    let engine = engine(&dir, "");
    let run = engine.check(&[], &[]).unwrap();
    let findings: Vec<_> = run.diagnostics.iter().collect();

    let shown = engine.preview_fixes(&stuck(), &findings);
    let report = engine.fix(&stuck(), &FixRun::default()).unwrap();

    assert_eq!(shown, [report.previews[0].preview.clone()]);
}
