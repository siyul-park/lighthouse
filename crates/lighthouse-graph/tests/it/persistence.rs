//! The same edits across reopen boundaries give the same results and the
//! same executed nodes as one long-lived graph.

use lighthouse_graph::Options;

use crate::{differential::*, toy::*};

/// Runs a sequence; the graph is dropped and opened again every `reopen`
/// steps when it is not zero.
type Trace = Vec<(Vec<(String, Facts)>, Vec<String>)>;

fn run(seed: u64, steps: usize, reopen: usize) -> Trace {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("cache/graph.db");
    let mut rng = Rng::new(seed);
    let mut sim = Sim::new();
    let mut graph = open(Some(&path), &sim.project);
    apply(&mut graph, &sim.project, &sim.known);
    pull(&graph);
    graph.commit().expect("commit");
    graph.take_executed();
    let mut trace = Vec::new();
    for step in 1..=steps {
        let edit = sim.step(&mut rng);
        if reopen != 0 && step % reopen == 0 {
            drop(graph);
            graph = open(Some(&path), &sim.project);
        }
        if matches!(edit, Edit::Evict) {
            for file in sim.project.files.keys() {
                graph.evict(&FACTS, file).expect("evict");
            }
        }
        apply(&mut graph, &sim.project, &sim.known);
        let report = pull(&graph);
        let mut ran = graph.take_executed();
        ran.sort();
        graph.commit().expect("commit");
        trace.push((report, ran));
    }
    trace
}

#[test]
fn reopening_changes_neither_results_nor_executed_nodes() {
    for seed in 1..=12 {
        let long = run(seed, 50, 0);
        assert_eq!(
            long,
            run(seed, 50, 1),
            "seed {seed} reopening at every step"
        );
        assert_eq!(
            long,
            run(seed, 50, 7),
            "seed {seed} reopening every 7 steps"
        );
    }
}

#[test]
fn a_new_process_finds_everything_green() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("graph.db");
    let mut sim = Sim::new();
    sim.step(&mut Rng::new(3));
    let mut graph = open(Some(&path), &sim.project);
    apply(&mut graph, &sim.project, &sim.known);
    let first = pull(&graph);
    graph.commit().expect("commit");
    drop(graph);

    let mut graph = open(Some(&path), &sim.project);
    apply(&mut graph, &sim.project, &sim.known);

    assert_eq!(pull(&graph), first);
    assert!(graph.take_executed().is_empty());
}

#[test]
fn a_changed_version_runs_its_readers_in_a_new_process() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("graph.db");
    let sim = Sim::new();
    let mut graph = open(Some(&path), &sim.project);
    apply(&mut graph, &sim.project, &sim.known);
    pull(&graph);
    graph.commit().expect("commit");
    drop(graph);
    let mut project = sim.project.clone();
    project.versions.insert("parser".into(), "p2".into());

    let mut graph = open(Some(&path), &project);
    apply(&mut graph, &project, &sim.known);
    pull(&graph);

    let ran = graph.take_executed();
    assert_eq!(
        ran.iter().filter(|label| kind_of(label) == "parse").count(),
        sim.known.len()
    );
}

#[test]
fn a_damaged_store_is_replaced() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("graph.db");
    std::fs::write(
        &path,
        b"this is not a database, and it is long enough to look like one at a glance",
    )
    .expect("write");
    let sim = Sim::new();

    let mut graph = open(Some(&path), &sim.project);
    apply(&mut graph, &sim.project, &sim.known);

    assert_eq!(pull(&graph), from_scratch(&sim.project, &sim.known));
}

#[test]
fn the_byte_limit_drops_values_and_keeps_results() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("graph.db");
    let sim = Sim::new();
    let options = Options {
        limit: 1,
        ..options(&sim.project)
    };
    let mut graph = open_with(Some(&path), options);
    apply(&mut graph, &sim.project, &sim.known);
    let first = pull(&graph);
    graph.commit().expect("commit");

    assert_eq!(pull(&graph), first);
    graph.take_executed();
    assert_eq!(pull(&graph), first);
    assert!(graph.take_executed().is_empty());
}

mod eviction_order {
    use lighthouse_graph::{Durability, Error, Graph, Input, Options, Query, VERSION};

    static SEED: Input<(), u8> = Input::new("seed");
    static STATIC_BIG: Query<(), String> = Query::new("static-big", |graph, _| {
        Ok(format!(
            "{}{}",
            graph.input(&VERSION, &"v".to_owned())?.unwrap_or_default(),
            "s".repeat(1000)
        ))
    });
    static VOLATILE_BIG: Query<(), String> = Query::new("volatile-big", |graph, _| {
        Ok(format!(
            "{}{}",
            graph.input(&SEED, &())?.unwrap_or_default(),
            "v".repeat(1000)
        ))
    });

    fn pull(graph: &Graph) -> Result<(), Error> {
        graph.query(&STATIC_BIG, &())?;
        graph.query(&VOLATILE_BIG, &())?;
        Ok(())
    }

    #[test]
    fn volatile_values_go_before_static_ones() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("graph.db");
        let options = Options {
            versions: [("v".to_owned(), "1".to_owned())].into(),
            limit: 1500,
            verify: 0,
            ..Options::default()
        };
        let mut graph = Graph::open(&path, options.clone()).expect("open");
        graph
            .set_input(&SEED, &(), 1, Durability::Volatile)
            .expect("set");
        pull(&graph).expect("pull");
        graph.commit().expect("commit");
        graph.take_executed();

        drop(graph);
        let mut graph = Graph::open(&path, options).expect("open");
        graph
            .set_input(&SEED, &(), 1, Durability::Volatile)
            .expect("set");
        pull(&graph).expect("pull");

        assert_eq!(graph.take_executed(), ["volatile-big(())"]);
    }
}

/// How many revisions a row is kept in this test: small, so that the file is
/// seen to stop growing within a few cycles.
const RETAIN: u64 = 8;

fn rows(path: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(path).expect("open the file");
    conn.query_row("SELECT count(*) FROM node", [], |row| row.get(0))
        .expect("count")
}

#[test]
fn rows_of_files_that_are_gone_are_deleted_so_the_file_does_not_grow() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("graph.db");
    let mut project = Project::new();
    let options = Options {
        retain: RETAIN,
        ..options(&project)
    };
    let mut graph = open_with(Some(&path), options);
    let mut counts = Vec::new();
    for cycle in 0..60 {
        let name = format!("f{cycle}");
        project
            .files
            .insert(name.clone(), format!("api=A{cycle};body=B;uses="));
        if cycle > 0 {
            project.files.remove(&format!("f{}", cycle - 1));
        }
        let known: Vec<String> = project.files.keys().cloned().collect();
        apply(&mut graph, &project, &known);
        pull(&graph);
        graph.commit().expect("commit");
        counts.push(rows(&path));
    }

    let (middle, last) = (counts[30], counts[59]);
    assert!(
        last <= middle,
        "{middle} rows at 30, {last} at 59: {counts:?}"
    );
    assert!(
        last < 60 * 5,
        "the rows of 60 files are not all kept: {last}"
    );
    assert_eq!(
        pull(&graph),
        from_scratch(&project, &project.files.keys().cloned().collect::<Vec<_>>())
    );
}

mod registration {
    use lighthouse_graph::{Durability, Graph, Input, Options, Query};

    static SEED: Input<(), u8> = Input::new("seed");
    static OTHER: Input<(), u8> = Input::new("other");
    static LOW: Query<(), u8> = Query::new("low", |graph, _| {
        Ok(graph.input(&SEED, &())?.map_or(0, |seed| *seed) + 1)
    });
    static HIGH: Query<(), u8> = Query::new("high", |graph, _| Ok(*graph.query(&LOW, &())? + 1));

    /// Opens the file with `other` set, a revision of its own when it differs.
    fn reopen(path: &std::path::Path, other: u8) -> Graph {
        let options = Options {
            verify: 0,
            ..Options::default()
        };
        let mut graph = Graph::open(path, options).expect("open");
        graph
            .set_input(&SEED, &(), 1, Durability::Volatile)
            .expect("set");
        graph
            .set_input(&OTHER, &(), other, Durability::Volatile)
            .expect("set");
        graph
    }

    #[test]
    fn a_stored_dependency_of_a_kind_not_used_yet_is_changed_never_green() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("graph.db");
        let mut graph = reopen(&path, 1);
        graph.query(&HIGH, &()).expect("pull");
        graph.commit().expect("commit");
        drop(graph);

        let mut graph = reopen(&path, 2);
        graph.query(&HIGH, &()).expect("pull");
        assert_eq!(graph.take_executed(), ["high(())"], "low was not used yet");
        graph.commit().expect("commit");
        drop(graph);

        let mut graph = reopen(&path, 3);
        graph.query(&LOW, &()).expect("pull");
        graph.query(&HIGH, &()).expect("pull");
        assert!(graph.take_executed().is_empty(), "low was used first");
        graph.commit().expect("commit");
    }
}
