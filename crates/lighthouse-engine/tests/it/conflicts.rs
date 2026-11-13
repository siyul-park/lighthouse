//! The search for contradictions between decisions starts only for a run in
//! which a rule reads `decision.conflicts`.

use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use lighthouse_engine::{Conflicts, Engine};
use lighthouse_spec::Config;
use lighthouse_test_support::{bundled, registry};

const READING: &str = "core/no-contradictory-decisions";
const NOT_READING: &str = "core/decision-naming";

/// An engine over a project that writes one decision of the bundled catalog,
/// with both decisions on, searching with a test that counts its questions.
fn engine() -> (Engine, Arc<AtomicUsize>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("decisions/design")).unwrap();
    fs::write(
        dir.path().join("decisions/design/top-down-order.yaml"),
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: design/top-down-order\n",
    )
    .unwrap();
    let config = Config::parse_inline(&format!(
        "plugins = [\"core\"]\n[rules.\"{READING}\"]\nlevel = \"error\"\n[rules.\"{NOT_READING}\"]\nlevel = \"warn\"\n"
    ))
    .unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asked);
    let conflicts = Conflicts::new(
        Arc::new(bundled().clone()),
        Arc::new(move |decision: &lighthouse_spec::Decision| {
            counted.fetch_add(1, Ordering::SeqCst);
            decision.id().starts_with("design/")
        }),
    );
    let engine = Engine::new(registry(), config, bundled(), dir.path())
        .unwrap()
        .with_conflicts(conflicts);
    (engine, asked, dir)
}

#[test]
fn a_run_without_the_decision_never_starts_the_search() {
    let (engine, asked, _dir) = engine();

    let run = engine.check(&[], &[NOT_READING.to_owned()]).unwrap();

    assert!(run.diagnostics.is_empty());
    assert_eq!(asked.load(Ordering::SeqCst), 0);
}

#[test]
fn a_run_with_the_decision_starts_the_search() {
    let (engine, asked, _dir) = engine();

    let run = engine.check(&[], &[READING.to_owned()]).unwrap();

    assert!(run.diagnostics.is_empty());
    assert!(asked.load(Ordering::SeqCst) > 0);
}

#[test]
fn engine_with_conflicts_keeps_the_search_it_is_given() {
    let (engine, asked, _dir) = engine();

    let run = engine.check(&[], &[READING.to_owned()]).unwrap();

    assert!(run.notices.is_empty(), "{:?}", run.notices);
    assert!(asked.load(Ordering::SeqCst) > 0);
}

#[test]
fn a_search_that_cannot_be_done_in_full_is_a_notice_naming_the_language_and_why() {
    let (engine, _asked, _dir) = engine();
    let everything = Conflicts::new(Arc::new(bundled().clone()), Arc::new(|_| true));
    let engine = engine.with_conflicts(everything);

    let run = engine.check(&[], &[READING.to_owned()]).unwrap();

    let told = run.notices.iter().any(|n| {
        n.starts_with("the search for contradictions among ")
            && n.contains("decisions is incomplete: no renderer for")
    });
    assert!(told, "{:?}", run.notices);
}
