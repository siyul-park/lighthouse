//! Decisions are identified by uid: findings and verdicts recorded under the
//! fingerprint a decision's name seeded move to the one its uid seeds, and a
//! verdict that is never matched again is still the decision's.

use std::fs;

use crate::support::*;
use lighthouse_model::{Reason, Verdict};
use lighthouse_store::{Filter, Observed, Standing, StatusFilter, Store};

/// A finding the way the first run after a decision gained its uid sees it:
/// a new fingerprint, and the one the decision's name seeded.
fn adopted(fingerprint: &str, legacy: &str, uid: &str) -> Observed {
    Observed {
        legacy_fingerprints: vec![legacy.to_owned()],
        decision_uid: Some(uid.to_owned()),
        ..observed(fingerprint, "design/a", "a.go")
    }
}

#[test]
fn a_finding_and_its_verdicts_move_to_the_fingerprint_the_uid_seeds() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![observed("legacy1", "design/a", "a.go")]))
        .unwrap();
    judge(
        &mut store,
        "legacy1",
        Verdict::Rejected,
        Reason::FalsePositive,
    );
    let before = store.finding("legacy1").unwrap();
    assert_eq!(standing_of(&store, "legacy1"), Some(Standing::Suppressed));

    let summary = store
        .record(&run(vec![adopted("new1", "legacy1", "uid-a")]))
        .unwrap();

    assert_eq!((summary.opened, summary.resolved), (0, 0), "{summary:?}");
    assert_eq!(summary.rewritten, 2, "the row and the verdicts");
    let after = store.finding("new1").unwrap();
    assert_eq!(after.first_seen, before.first_seen, "the history stays");
    assert_eq!(after.decision_uid.as_deref(), Some("uid-a"));
    assert_eq!(standing_of(&store, "new1"), Some(Standing::Suppressed));
    assert_eq!(store.history("new1").unwrap().len(), 1);
    assert_eq!(
        store
            .list(&Filter {
                rule: None,
                status: StatusFilter::All,
            })
            .unwrap()
            .into_iter()
            .map(|f| f.fingerprint)
            .collect::<Vec<_>>(),
        ["new1"],
        "no orphaned row under the legacy fingerprint"
    );
    assert_eq!(rewrites_in(dir.path()), 1);

    let again = store
        .record(&run(vec![adopted("new1", "legacy1", "uid-a")]))
        .unwrap();
    assert_eq!(again.rewritten, 0, "adopting twice changes nothing");
    assert_eq!(rewrites_in(dir.path()), 1);
}

#[test]
fn a_rewrite_in_the_log_applies_to_a_cache_rebuilt_from_it() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut store = Store::open(dir.path()).unwrap();
        store
            .record(&run(vec![observed("legacy1", "design/a", "a.go")]))
            .unwrap();
        judge(
            &mut store,
            "legacy1",
            Verdict::Rejected,
            Reason::FalsePositive,
        );
        store
            .record(&run(vec![adopted("new1", "legacy1", "uid-a")]))
            .unwrap();
    }
    fs::remove_file(Store::path_in(dir.path())).unwrap();

    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![observed("new1", "design/a", "a.go")]))
        .unwrap();

    assert_eq!(standing_of(&store, "new1"), Some(Standing::Suppressed));
    assert_eq!(store.history("new1").unwrap().len(), 1);
}

#[test]
fn a_verdict_never_matched_again_stays_readable_through_the_uid() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![observed("gone", "design/old-name", "a.go")]))
        .unwrap();
    judge(&mut store, "gone", Verdict::Confirmed, Reason::Fixed);
    assert_eq!(store.history("gone").unwrap()[0].decision_uid, None);

    store
        .identify(
            &[
                ("design/old-name".to_owned(), "uid-a".to_owned()),
                ("design/a".to_owned(), "uid-a".to_owned()),
            ]
            .into(),
        )
        .unwrap();

    let event = &store.history("gone").unwrap()[0];
    assert_eq!(event.decision_uid.as_deref(), Some("uid-a"));
    assert_eq!(
        event.rule_id, "design/old-name",
        "the name is kept to be read"
    );
    assert_eq!(
        store.finding("gone").unwrap().decision_uid.as_deref(),
        Some("uid-a")
    );
}

#[test]
fn a_verdict_records_the_uid_of_its_decision() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![adopted("f1", "legacy1", "uid-a")]))
        .unwrap();
    judge(&mut store, "f1", Verdict::Confirmed, Reason::Fixed);

    let line = log_of(dir.path());

    assert!(line.contains("\"decisionUid\":\"uid-a\""), "{line}");
    assert!(line.contains("\"ruleId\":\"design/a\""), "{line}");
    assert!(!line.contains("decisionName"), "{line}");
}

fn rewrites_in(dir: &std::path::Path) -> usize {
    log_of(dir).matches("\"kind\":\"Rewrite\"").count()
}

#[test]
fn a_cache_event_that_learned_its_uid_is_exported_as_it_was_written() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut store = Store::open(dir.path()).unwrap();
        store
            .record(&run(vec![observed("f1", "design/a", "a.go")]))
            .unwrap();
        judge(&mut store, "f1", Verdict::Confirmed, Reason::Fixed);
    }
    // The line is lost; only the cache has the event. It then learns the uid.
    fs::remove_file(Store::log_path_in(dir.path())).unwrap();
    {
        let mut store = Store::open(dir.path()).unwrap();
        store
            .identify(&[("design/a".to_owned(), "uid-a".to_owned())].into())
            .unwrap();
    }

    // Opening re-exports the event; the id still matches what is written.
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.history("f1").unwrap().len(), 1);
    let line = log_of(dir.path());
    assert!(line.contains("\"ruleId\":\"design/a\""), "{line}");
    assert!(!line.contains("decisionUid"), "written as it was: {line}");
    drop(store);
    let again = Store::open(dir.path()).unwrap();
    assert_eq!(again.history("f1").unwrap().len(), 1);
    assert_eq!(
        again.history("f1").unwrap()[0].decision_uid.as_deref(),
        Some("uid-a")
    );
}

#[test]
fn two_decisions_that_answer_to_one_old_fingerprint_do_not_share_its_verdicts() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store
        .record(&run(vec![observed("legacy1", "design/a", "a.go")]))
        .unwrap();
    judge(
        &mut store,
        "legacy1",
        Verdict::Rejected,
        Reason::FalsePositive,
    );
    store
        .record(&run(vec![adopted("new1", "legacy1", "uid-a")]))
        .unwrap();

    let second = store
        .record(&run(vec![
            adopted("new1", "legacy1", "uid-a"),
            adopted("new2", "legacy1", "uid-b"),
        ]))
        .unwrap();

    assert_eq!(second.rewritten, 0);
    assert_eq!(standing_of(&store, "new1"), Some(Standing::Suppressed));
    assert_eq!(
        standing_of(&store, "new2"),
        None,
        "the verdict stays with the first"
    );
    assert_eq!(rewrites_in(dir.path()), 1);
}

#[test]
fn a_log_that_moves_one_old_fingerprint_two_ways_is_refused_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut store = Store::open(dir.path()).unwrap();
        store
            .record(&run(vec![observed("legacy1", "design/a", "a.go")]))
            .unwrap();
        judge(
            &mut store,
            "legacy1",
            Verdict::Rejected,
            Reason::FalsePositive,
        );
        store
            .record(&run(vec![adopted("new1", "legacy1", "uid-a")]))
            .unwrap();
    }
    let log = log_of(dir.path());
    let rewrite = log
        .lines()
        .find(|l| l.contains("\"kind\":\"Rewrite\""))
        .unwrap();
    let other = rewrite.replace("new1", "new2");
    fs::write(Store::log_path_in(dir.path()), format!("{log}{other}\n")).unwrap();
    fs::remove_file(Store::path_in(dir.path())).unwrap();

    let store = Store::open(dir.path()).unwrap();

    assert!(
        store
            .notices()
            .iter()
            .any(|n| n.contains("already belong to new1")),
        "{:?}",
        store.notices()
    );
}
