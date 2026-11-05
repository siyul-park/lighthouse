//! Fan-out safety: queries that fan out with `Graph::par_map` over heavily shared
//! dependencies on a pool of two threads neither deadlock nor run a node
//! twice, and a cycle through fanned-out branches is an error.

use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use lighthouse_graph::{Error, Graph, Options, Query};
use rayon::ThreadPoolBuilder;

const LAYERS: u32 = 50;
const WIDTH: u32 = 250;
const FAN: u32 = 6;
const TOPS: u32 = 8;
const PER_TOP: u32 = 48;
const DEADLINE: Duration = Duration::from_secs(120);

type At = (u32, u32);

static NODE: Query<At, u64> = Query::new("node", |graph, at| fan(graph, *at, &NODE));
/// A small layered graph with a back edge from the node (2, 0) to (6, 0): a
/// cycle, entered from above. A failed node is not kept, so the graph is small
/// enough for every path through it to be run.
static BACK: Query<At, u64> = Query::new("back", back);
static TOP: Query<u32, u64> = Query::new("top", top_of);
static RING: Query<u32, u64> = Query::new("ring", |graph, at| {
    let next = vec![(at + 1) % 6, (at + 3) % 6];
    all(graph.par_map(next, |graph, to| graph.query(&RING, to).map(|v| *v)))
});

/// The dependencies of a node: shared by construction, as many nodes of a
/// layer pick the same few of the layer below.
fn deps((layer, i): At) -> Vec<At> {
    (0..FAN)
        .map(|k| (layer - 1, (i * 7 + k * 13 + layer * 3) % WIDTH))
        .collect()
}

fn mix(values: &[u64], at: At) -> u64 {
    values
        .iter()
        .fold(u64::from(at.0 * WIDTH + at.1), |acc, v| {
            acc.rotate_left(5).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ v
        })
}

fn all(results: Vec<Result<u64, Error>>) -> Result<u64, Error> {
    let mut sum = 0u64;
    for result in results {
        sum = sum.wrapping_add(result?);
    }
    Ok(sum)
}

fn fan(graph: &Graph, at: At, query: &'static Query<At, u64>) -> Result<u64, Error> {
    if at.0 == 0 {
        return Ok(u64::from(at.1) + 1);
    }
    let mut values = Vec::new();
    for result in graph.par_map(deps(at), move |graph, dep| {
        graph.query(query, dep).map(|v| *v)
    }) {
        values.push(result?);
    }
    Ok(mix(&values, at))
}

fn back(graph: &Graph, at: &At) -> Result<u64, Error> {
    let (layer, i) = *at;
    if layer == 0 {
        return Ok(1);
    }
    let mut wanted = vec![
        (layer - 1, i),
        (layer - 1, (i + 1) % 16),
        (layer - 1, (i + 5) % 16),
    ];
    if *at == (2, 0) {
        wanted.push((6, 0));
    }
    let mut sum = 0u64;
    for result in graph.par_map(wanted, |graph, dep| graph.query(&BACK, dep).map(|v| *v)) {
        sum = sum.wrapping_add(result?);
    }
    Ok(sum)
}

fn tops(top: u32) -> Vec<At> {
    (0..PER_TOP)
        .map(|k| (LAYERS - 1, (top * 31 + k * 5) % WIDTH))
        .collect()
}

fn top_of(graph: &Graph, top: &u32) -> Result<u64, Error> {
    all(graph.par_map(tops(*top), |graph, at| graph.query(&NODE, at).map(|v| *v)))
}

/// What the graph must give, by plain recursion, and the nodes it reaches.
fn reference(at: At, memo: &mut HashMap<At, u64>) -> u64 {
    if let Some(value) = memo.get(&at) {
        return *value;
    }
    let value = if at.0 == 0 {
        u64::from(at.1) + 1
    } else {
        let values: Vec<u64> = deps(at).into_iter().map(|d| reference(d, memo)).collect();
        mix(&values, at)
    };
    memo.insert(at, value);
    value
}

fn small_pool() -> rayon::ThreadPool {
    ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .expect("pool")
}

/// Runs `work` on its own thread so that a deadlock fails the test instead of
/// hanging it.
fn within_deadline<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver
        .recv_timeout(DEADLINE)
        .expect("the pulls finished: no deadlock")
}

fn graph() -> Arc<Graph> {
    Arc::new(
        Graph::in_memory(Options {
            verify: 0,
            ..Options::default()
        })
        .expect("open"),
    )
}

#[test]
fn fan_out_over_shared_dependencies_on_two_threads_runs_each_node_once() {
    let graph = graph();
    let pool = Arc::new(small_pool());

    let values = within_deadline({
        let (graph, pool) = (graph.clone(), pool.clone());
        move || {
            let pulls: Vec<_> = (0..TOPS)
                .map(|top| {
                    let (graph, pool) = (graph.clone(), pool.clone());
                    thread::spawn(move || pool.install(|| *graph.query(&TOP, &top).expect("top")))
                })
                .collect();
            pulls
                .into_iter()
                .map(|pull| pull.join().expect("pull"))
                .collect::<Vec<u64>>()
        }
    });

    let mut memo = HashMap::new();
    for top in 0..TOPS {
        let expected = tops(top)
            .into_iter()
            .fold(0u64, |sum, at| sum.wrapping_add(reference(at, &mut memo)));
        assert_eq!(values[top as usize], expected, "top {top}");
    }
    let ran = graph.take_executed();
    let nodes: Vec<&String> = ran
        .iter()
        .filter(|label| label.starts_with("node("))
        .collect();
    let distinct: BTreeSet<&&String> = nodes.iter().collect();
    assert!(nodes.len() >= 10_000, "{} nodes", nodes.len());
    assert_eq!((distinct.len(), nodes.len()), (memo.len(), memo.len()));
}

#[test]
fn a_cycle_through_fanned_out_branches_is_an_error() {
    let graph = graph();
    let pool = Arc::new(small_pool());

    let results = within_deadline({
        let (graph, pool) = (graph.clone(), pool.clone());
        move || {
            let pulls: Vec<_> = (0..12)
                .map(|n| {
                    let (graph, pool) = (graph.clone(), pool.clone());
                    let top = [(7, 0), (7, 15), (7, 11)][n % 3];
                    thread::spawn(move || pool.install(|| graph.query(&BACK, &top).map(|v| *v)))
                })
                .collect();
            pulls
                .into_iter()
                .map(|pull| pull.join().expect("pull"))
                .collect::<Vec<_>>()
        }
    });

    for result in results {
        let Err(Error::Cycle(path)) = result else {
            panic!("expected a cycle, got {result:?}");
        };
        assert!(path.len() >= 3, "{path:?}");
        assert_eq!(path.first(), path.last());
    }
}

#[test]
fn a_ring_of_fanned_out_queries_is_a_cycle_from_every_entry() {
    let graph = graph();
    let pool = Arc::new(small_pool());

    let results = within_deadline({
        let (graph, pool) = (graph.clone(), pool.clone());
        move || {
            let pulls: Vec<_> = (0..6)
                .map(|at| {
                    let (graph, pool) = (graph.clone(), pool.clone());
                    thread::spawn(move || pool.install(|| graph.query(&RING, &at).map(|v| *v)))
                })
                .collect();
            pulls
                .into_iter()
                .map(|pull| pull.join().expect("pull"))
                .collect::<Vec<_>>()
        }
    });

    assert!(
        results
            .iter()
            .all(|result| matches!(result, Err(Error::Cycle(_)))),
        "{results:?}"
    );
}
