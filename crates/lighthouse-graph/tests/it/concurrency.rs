use std::{sync::mpsc, thread, time::Duration};

use lighthouse_graph::{Db, Durability, Error, Input, Query};
use rayon::prelude::*;

static N: Input<u64, u64> = Input::new("n");
/// `sum(i)` is the sum of the inputs `0..=i`, built from `sum(i - 1)`.
static SUM: Query<u64, u64> = Query::new("sum", sum);
static CYCLE_A: Query<u64, u64> =
    Query::new("cycle-a", |db, key| Ok(*db.query(&CYCLE_B, key)? + 1));
static CYCLE_B: Query<u64, u64> =
    Query::new("cycle-b", |db, key| Ok(*db.query(&CYCLE_A, key)? + 1));
static SLOW_X: Query<(), u64> = Query::new("slow-x", |db, _| {
    thread::sleep(Duration::from_millis(100));
    Ok(*db.query(&SLOW_Y, &())?)
});
static SLOW_Y: Query<(), u64> = Query::new("slow-y", |db, _| {
    thread::sleep(Duration::from_millis(100));
    Ok(*db.query(&SLOW_X, &())?)
});

fn sum(db: &Db, key: &u64) -> Result<u64, Error> {
    let own = *db.input(&N, key)?;
    if *key == 0 {
        return Ok(own);
    }
    Ok(own + *db.query(&SUM, &(key - 1))?)
}

fn graph(count: u64) -> Db {
    let mut db = Db::in_memory(&Default::default()).expect("open");
    db.set_verify(0);
    for key in 0..count {
        db.set_input(&N, &key, key, Durability::Volatile)
            .expect("set");
    }
    db
}

#[test]
fn parallel_pulls_of_overlapping_keys_agree_and_run_each_node_once() {
    let db = graph(300);
    let keys: Vec<u64> = (0..2000).map(|i| (i * 7919) % 300).collect();

    let sums: Vec<u64> = keys
        .par_iter()
        .map(|key| *db.query(&SUM, key).expect("sum"))
        .collect();

    assert!(
        keys.iter()
            .zip(&sums)
            .all(|(key, sum)| *sum == key * (key + 1) / 2)
    );
    let mut ran = db.take_executed();
    ran.sort();
    let before = ran.len();
    ran.dedup();
    assert_eq!((ran.len(), before), (300, 300));
}

#[test]
fn a_cycle_is_an_error_naming_the_queries() {
    let db = graph(0);

    let pulled = db.query(&CYCLE_A, &1);

    let Err(Error::Cycle(path)) = pulled else {
        panic!("expected a cycle, got {pulled:?}");
    };
    assert_eq!(path, ["cycle-a(1)", "cycle-b(1)", "cycle-a(1)"]);
}

#[test]
fn a_cycle_across_threads_is_an_error_and_not_a_deadlock() {
    let db = std::sync::Arc::new(graph(0));
    let (sender, receiver) = mpsc::channel();
    for pull in [0, 1] {
        let (db, sender) = (db.clone(), sender.clone());
        thread::spawn(move || {
            let result = if pull == 0 {
                db.query(&SLOW_X, &())
            } else {
                db.query(&SLOW_Y, &())
            };
            let _ = sender.send(result.is_err());
        });
    }
    let first = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("a pull returned");
    assert!(first);
    assert!(
        receiver.recv_timeout(Duration::from_secs(10)).is_ok(),
        "the second pull must not hang"
    );
}
