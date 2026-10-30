//! Opening the cache: one built from the log, shared by processes.

use std::{fs, thread};

use crate::support::*;
use lighthouse_model::Judgment;
use lighthouse_store::{Error, Store};
use rusqlite::Connection;

#[test]
fn store_opens_a_cache_and_builds_one_of_another_version_again_from_the_log() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Store::path_in(dir.path()).ends_with(".lighthouse/lighthouse.db"));
    assert!(Store::log_path_in(dir.path()).ends_with(".lighthouse/decisions.jsonl"));
    assert!(Store::open_existing(dir.path()).unwrap().is_none());

    let mut first = Store::open(dir.path()).unwrap();
    assert_eq!(first.schema_version().unwrap(), 12);
    first
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    judge(&mut first, "f1", Judgment::Pass);
    drop(first);
    let again = Store::open_existing(dir.path()).unwrap().unwrap();
    assert_eq!(again.schema_version().unwrap(), 12);
    assert_eq!(again.finding("f1").unwrap().judgment, Some(Judgment::Pass));
    drop(again);

    for version in [99, 3] {
        Connection::open(Store::path_in(dir.path()))
            .unwrap()
            .pragma_update(None, "user_version", version)
            .unwrap();
        let rebuilt = Store::open(dir.path()).unwrap();
        assert_eq!(rebuilt.schema_version().unwrap(), 12, "from {version}");
        assert!(
            matches!(rebuilt.finding("f1"), Err(Error::UnknownFinding(_))),
            "the sightings of a cache of another version are gone"
        );
        assert_eq!(
            rebuilt.history("f1").unwrap().len(),
            1,
            "the judgments come back from the log"
        );
    }
}

#[test]
fn a_garbage_cache_is_named_and_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = Store::path_in(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "this is not a database, it is only a long enough line of text ".repeat(40),
    )
    .unwrap();
    let error = Store::open(dir.path()).err().unwrap();
    assert!(matches!(error, Error::Corrupt { .. }), "{error}");
    let message = error.to_string();
    assert!(message.contains(&path.display().to_string()), "{message}");
    assert!(message.contains("nothing was deleted"), "{message}");
    assert!(path.is_file());
}

#[test]
fn processes_opening_a_fresh_cache_together_all_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let root = root.clone();
            thread::spawn(move || Store::open(&root).unwrap().schema_version().unwrap())
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), 12);
    }
}

#[test]
fn two_stores_on_one_cache_can_record_and_resolve_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut seed = Store::open(dir.path()).unwrap();
    seed.record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    drop(seed);

    let root = dir.path().to_owned();
    let recorder = {
        let root = root.clone();
        thread::spawn(move || {
            let mut store = Store::open(&root).unwrap();
            for _ in 0..25 {
                store
                    .record(&run(vec![observed("f1", "design/a", "a.go")]))
                    .unwrap();
            }
        })
    };
    let judge = thread::spawn(move || {
        let mut store = Store::open(&root).unwrap();
        for _ in 0..25 {
            store
                .resolve(&review("f1", Judgment::Fail), stamp("sem1"))
                .unwrap();
        }
    });
    recorder.join().unwrap();
    judge.join().unwrap();
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.history("f1").unwrap().len(), 25);
}

#[test]
fn a_cache_with_tables_and_no_version_is_built_again_from_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = Store::open(dir.path()).unwrap();
    first
        .record(&run(vec![observed("f1", "design/a", "a.go")]))
        .unwrap();
    judge(&mut first, "f1", Judgment::Pass);
    drop(first);
    Connection::open(Store::path_in(dir.path()))
        .unwrap()
        .pragma_update(None, "user_version", 0)
        .unwrap();

    let rebuilt = Store::open(dir.path()).unwrap();

    assert_eq!(rebuilt.schema_version().unwrap(), 12);
    assert_eq!(rebuilt.history("f1").unwrap().len(), 1, "from the log");
    assert!(rebuilt.list(&all()).unwrap().is_empty());
}
