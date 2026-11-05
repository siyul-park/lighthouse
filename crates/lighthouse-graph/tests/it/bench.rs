//! Timings, printed and never asserted. `GRAPH_BENCH_N=100000 cargo test
//! --release -p lighthouse-graph bench -- --nocapture` for the full size.

use std::time::Instant;

use lighthouse_graph::{Durability, Error, Graph, Input, Options, Query};

static LEAF_IN: Input<u32, String> = Input::new("leaf-in");
static LEAF: Query<u32, usize> = Query::new("leaf", |graph, key| {
    Ok(graph.input(&LEAF_IN, key)?.map_or(0, |leaf| leaf.len()))
});
static TOTAL: Query<u32, usize> = Query::new("total", total);

fn total(graph: &Graph, count: &u32) -> Result<usize, Error> {
    let mut all = 0;
    for key in 0..*count {
        all += *graph.query(&LEAF, &key)?;
    }
    Ok(all)
}

fn size() -> u32 {
    std::env::var("GRAPH_BENCH_N")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(10_000)
}

fn open(path: &std::path::Path) -> Graph {
    let options = Options {
        verify: 0,
        ..Options::default()
    };
    Graph::open(path, options).expect("open")
}

fn set_all(graph: &mut Graph, count: u32, edited: Option<u32>) {
    for key in 0..count {
        let text = if edited == Some(key) {
            "edited text".to_owned()
        } else {
            format!("text {key}")
        };
        graph
            .set_input(&LEAF_IN, &key, text, Durability::Volatile)
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
    let mut graph = open(&path);
    set_all(&mut graph, count, None);
    let cold = timed("cold run", count, || {
        *graph.query(&TOTAL, &count).expect("total")
    });
    timed("commit", count, || graph.commit().expect("commit"));
    graph.take_executed();

    set_all(&mut graph, count, Some(0));
    let edited = timed("validate all green after one input change", count, || {
        *graph.query(&TOTAL, &count).expect("total")
    });
    assert_eq!(graph.take_executed().len(), 2);
    assert_eq!(edited, cold - "text 0".len() + "edited text".len());
    timed("commit after the change", count, || {
        graph.commit().expect("commit")
    });
    drop(graph);

    let mut graph = open(&path);
    let reopened = timed("reopen and validate all green", count, || {
        set_all(&mut graph, count, Some(0));
        *graph.query(&TOTAL, &count).expect("total")
    });
    assert_eq!(reopened, edited);
    assert!(graph.take_executed().is_empty());
}
