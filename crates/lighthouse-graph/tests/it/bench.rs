//! Timings, printed and never asserted. `GRAPH_BENCH_N=100000 cargo test
//! --release -p lighthouse-graph bench -- --nocapture` for the full size.

use std::time::Instant;

use lighthouse_graph::{Db, Durability, Error, Input, Query};

static LEAF_IN: Input<u32, String> = Input::new("leaf-in");
static LEAF: Query<u32, usize> = Query::new("leaf", |db, key| Ok(db.input(&LEAF_IN, key)?.len()));
static TOTAL: Query<u32, usize> = Query::new("total", total);

fn total(db: &Db, count: &u32) -> Result<usize, Error> {
    let mut all = 0;
    for key in 0..*count {
        all += *db.query(&LEAF, &key)?;
    }
    Ok(all)
}

fn size() -> u32 {
    std::env::var("GRAPH_BENCH_N")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(10_000)
}

fn open(path: &std::path::Path) -> Db {
    let mut db = Db::open(path, &Default::default()).expect("open");
    db.set_verify(0);
    db.register_input(&LEAF_IN);
    db.register_query(&LEAF);
    db.register_query(&TOTAL);
    db
}

fn set_all(db: &mut Db, count: u32, edited: Option<u32>) {
    for key in 0..count {
        let text = if edited == Some(key) {
            "edited text".to_owned()
        } else {
            format!("text {key}")
        };
        db.set_input(&LEAF_IN, &key, text, Durability::Volatile)
            .expect("set");
    }
}

fn timed<T>(label: &str, count: u32, work: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let result = work();
    println!("bench {label} ({count} nodes): {:?}", started.elapsed());
    result
}

#[test]
fn bench_validation_and_reopen() {
    let count = size();
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("graph.db");
    let mut db = open(&path);
    set_all(&mut db, count, None);
    let cold = timed("cold run", count, || {
        *db.query(&TOTAL, &count).expect("total")
    });
    timed("commit", count, || db.commit().expect("commit"));
    db.take_executed();

    set_all(&mut db, count, Some(0));
    let edited = timed("validate all green after one input change", count, || {
        *db.query(&TOTAL, &count).expect("total")
    });
    assert_eq!(db.take_executed().len(), 2);
    assert_eq!(edited, cold - "text 0".len() + "edited text".len());
    timed("commit after the change", count, || {
        db.commit().expect("commit")
    });
    drop(db);

    let mut db = open(&path);
    let reopened = timed("reopen and validate all green", count, || {
        set_all(&mut db, count, Some(0));
        *db.query(&TOTAL, &count).expect("total")
    });
    assert_eq!(reopened, edited);
    assert!(db.take_executed().is_empty());
}
