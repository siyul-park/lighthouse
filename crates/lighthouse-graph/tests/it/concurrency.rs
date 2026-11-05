use std::{sync::mpsc, thread, time::Duration};

use lighthouse_graph::{Durability, Error, Graph, Input, Options, Query};
use rayon::prelude::*;

static N: Input<u64, u64> = Input::new("n");
/// `sum(i)` is the sum of the inputs `0..=i`, built from `sum(i - 1)`.
static SUM: Query<u64, u64> = Query::new("sum", sum);
static CYCLE_A: Query<u64, u64> =
    Query::new("cycle-a", |graph, key| Ok(*graph.query(&CYCLE_B, key)? + 1));
static CYCLE_B: Query<u64, u64> =
    Query::new("cycle-b", |graph, key| Ok(*graph.query(&CYCLE_A, key)? + 1));
static SLOW_X: Query<(), u64> = Query::new("slow-x", |graph, _| {
    thread::sleep(Duration::from_millis(100));
    Ok(*graph.query(&SLOW_Y, &())?)
});
static SLOW_Y: Query<(), u64> = Query::new("slow-y", |graph, _| {
    thread::sleep(Duration::from_millis(100));
    Ok(*graph.query(&SLOW_X, &())?)
});

fn sum(graph: &Graph, key: &u64) -> Result<u64, Error> {
    let own = graph.input(&N, key)?.map_or(0, |n| *n);
    if *key == 0 {
        return Ok(own);
    }
    Ok(own + *graph.query(&SUM, &(key - 1))?)
}

fn graph(count: u64) -> Graph {
    let mut graph = Graph::in_memory(Options {
        verify: 0,
        ..Options::default()
    })
    .expect("open");
    for key in 0..count {
        graph
            .set_input(&N, &key, key, Durability::Volatile)
            .expect("set");
    }
    graph
}

#[test]
fn parallel_pulls_of_overlapping_keys_agree_and_run_each_node_once() {
    let graph = graph(300);
    let keys: Vec<u64> = (0..2000).map(|i| (i * 7919) % 300).collect();

    let sums: Vec<u64> = keys
        .par_iter()
        .map(|key| *graph.query(&SUM, key).expect("sum"))
        .collect();

    assert!(
        keys.iter()
            .zip(&sums)
            .all(|(key, sum)| *sum == key * (key + 1) / 2)
    );
    let mut ran = graph.take_executed();
    ran.sort();
    let before = ran.len();
    ran.dedup();
    assert_eq!((ran.len(), before), (300, 300));
}

#[test]
fn a_cycle_is_an_error_naming_the_queries() {
    let graph = graph(0);

    let pulled = graph.query(&CYCLE_A, &1);

    let Err(Error::Cycle(path)) = pulled else {
        panic!("expected a cycle, got {pulled:?}");
    };
    assert_eq!(path, ["cycle-a(1)", "cycle-b(1)", "cycle-a(1)"]);
}

#[test]
fn a_cycle_across_threads_is_an_error_and_not_a_deadlock() {
    let graph = std::sync::Arc::new(graph(0));
    let (sender, receiver) = mpsc::channel();
    for pull in [0, 1] {
        let (graph, sender) = (graph.clone(), sender.clone());
        thread::spawn(move || {
            let result = if pull == 0 {
                graph.query(&SLOW_X, &())
            } else {
                graph.query(&SLOW_Y, &())
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
