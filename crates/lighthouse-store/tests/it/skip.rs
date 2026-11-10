//! A run equal to the last recorded run only refreshes the sightings.

use std::{thread, time::Duration};

use crate::support::*;
use lighthouse_store::{Run, RunSummary, State, Store, Unchecked};
use rusqlite::Connection;

/// A store on disk with a run recorded, and a connection that can tamper with
/// the rows: a deleted finding comes back only if the next run reads them.
fn recorded(first: &Run) -> (tempfile::TempDir, Store, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.record(first).unwrap();
    let raw = Connection::open(Store::path_in(dir.path())).unwrap();
    (dir, store, raw)
}

fn forget(raw: &Connection, fingerprint: &str) {
    raw.execute("DELETE FROM findings WHERE fingerprint = ?1", [fingerprint])
        .unwrap();
}

/// How a run is made to differ.
type Change = fn(&mut Run);

fn base() -> Run {
    run(vec![
        observed("f1", "design/a", "a.go"),
        observed("f2", "design/b", "b.go"),
    ])
}

#[test]
fn an_equal_run_skips_the_rows_and_refreshes_the_sightings() {
    let first = base();
    let (_dir, mut store, raw) = recorded(&first);
    let before = store.finding("f2").unwrap();
    forget(&raw, "f1");
    thread::sleep(Duration::from_millis(5));

    let again = Run {
        commit: Some("def456".to_owned()),
        dirty: false,
        ..first
    };
    assert_eq!(store.record(&again).unwrap(), RunSummary::default());

    assert!(
        store.finding("f1").is_err(),
        "the findings were not read back"
    );
    let after = store.finding("f2").unwrap();
    assert!(after.last_seen > before.last_seen);
    assert_eq!(after.commit.as_deref(), Some("def456"));
    assert_eq!(after.dirty, Some(false));
    let mut unchanged = after.clone();
    unchanged.last_seen = before.last_seen.clone();
    unchanged.commit = before.commit.clone();
    unchanged.dirty = before.dirty;
    assert_eq!(unchanged, before);
}

#[test]
fn a_run_that_differs_in_any_part_takes_the_full_path() {
    let variants: Vec<(&str, Change)> = vec![
        ("a finding", |r| r.observed[1].message = "other".to_owned()),
        ("another finding", |r| {
            r.observed.push(observed("f3", "design/a", "c.go"))
        }),
        ("reported", |r| r.reported = vec!["a.go".to_owned()]),
        ("ran", |r| r.ran = vec!["design/a".to_owned()]),
        ("active", |r| r.active = vec!["design/a".to_owned()]),
        ("unchecked paths", |r| {
            r.unchecked = Unchecked::Paths(vec!["x".to_owned()])
        }),
        ("unchecked everything", |r| {
            r.unchecked = Unchecked::Everything
        }),
    ];
    for (part, change) in variants {
        let first = base();
        let (_dir, mut store, raw) = recorded(&first);
        forget(&raw, "f1");
        let mut next = first;
        change(&mut next);
        store.record(&next).unwrap();
        assert!(store.finding("f1").is_ok(), "{part} was read back");
    }
}

#[test]
fn the_order_and_repeats_of_a_run_do_not_change_it() {
    let first = base();
    let (_dir, mut store, raw) = recorded(&first);
    forget(&raw, "f1");
    let mut same = first;
    same.observed.reverse();
    same.ran.reverse();
    same.ran.push("design/a".to_owned());
    store.record(&same).unwrap();
    assert!(store.finding("f1").is_err());
}

#[test]
fn a_finding_resolves_after_a_skipped_run() {
    let first = base();
    let (_dir, mut store, _raw) = recorded(&first);
    store.record(&first).unwrap();
    let gone = run(vec![observed("f1", "design/a", "a.go")]);
    assert_eq!(store.record(&gone).unwrap().resolved, 1);
    assert_eq!(store.finding("f2").unwrap().state, State::Resolved);
    assert_eq!(store.finding("f1").unwrap().state, State::Open);
    // The resolved finding is not refreshed by the run that equals the last.
    let seen = store.finding("f2").unwrap().last_seen;
    thread::sleep(Duration::from_millis(5));
    store.record(&gone).unwrap();
    assert_eq!(store.finding("f2").unwrap().last_seen, seen);
}

#[test]
fn prune_deletes_the_sightings_of_the_findings_it_deletes() {
    let first = base();
    let (_dir, mut store, raw) = recorded(&first);
    let count = || -> i64 {
        raw.query_row("SELECT count(*) FROM sightings", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(count(), 2);
    store
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    assert_eq!(store.prune(None).unwrap(), 1);
    assert_eq!(count(), 1);
}
