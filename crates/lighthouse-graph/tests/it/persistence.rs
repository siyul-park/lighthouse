//! The same edits across reopen boundaries give the same results and the
//! same executed nodes as one long-lived graph.

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
            graph.commit().expect("commit");
            drop(graph);
            let mut evicting = open(Some(&path), &sim.project);
            lighthouse_graph::trim(&mut evicting, u64::MAX).expect("trim");
            drop(evicting);
            graph = open(Some(&path), &sim.project);
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
fn trimming_drops_values_and_keeps_results() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("graph.db");
    let sim = Sim::new();
    let mut graph = open(Some(&path), &sim.project);
    apply(&mut graph, &sim.project, &sim.known);
    let first = pull(&graph);
    graph.commit().expect("commit");
    lighthouse_graph::trim(&mut graph, u64::MAX).expect("trim");

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
            verify: 0,
            ..Options::default()
        };
        let mut graph = Graph::open(&path, options.clone()).expect("open");
        graph
            .set_input(&SEED, &(), 1, Durability::Volatile)
            .expect("set");
        pull(&graph).expect("pull");
        graph.commit().expect("commit");
        lighthouse_graph::trim(&mut graph, 1).expect("trim");
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
    let mut graph = open(Some(&path), &project);
    let mut counts = Vec::new();
    for cycle in 0..200 {
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

    let (middle, last) = (counts[100], counts[199]);
    assert!(
        last <= middle,
        "{middle} rows at 100, {last} at 199: {counts:?}"
    );
    assert!(
        last < 100 * 5,
        "the rows of 100 files are not all kept: {last}"
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

mod persisted {
    use lighthouse_graph::{Durability, Graph, Input, Options, Query};

    static FRAGMENT: Input<String, String> = Input::new("fragment").persisted();
    static TRANSIENT: Input<String, String> = Input::new("transient");
    static SEEN: Query<String, String> = Query::new("seen", |graph, name| {
        let fragment = graph.input(&FRAGMENT, name)?;
        let transient = graph.input(&TRANSIENT, name)?;
        Ok(format!(
            "{}/{}",
            fragment.as_deref().map_or("-", String::as_str),
            transient.as_deref().map_or("-", String::as_str)
        ))
    });

    fn reopen(path: &std::path::Path) -> Graph {
        let options = Options {
            verify: 0,
            ..Options::default()
        };
        Graph::open(path, options).expect("open")
    }

    #[test]
    fn a_persisted_input_is_read_after_reopen_and_an_ordinary_one_is_not() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("graph.db");
        let name = "a".to_owned();
        let mut graph = reopen(&path);
        for input in [&FRAGMENT, &TRANSIENT] {
            graph
                .set_input(input, &name, "one".to_owned(), Durability::Volatile)
                .expect("set");
        }
        assert_eq!(*graph.query(&SEEN, &name).expect("pull"), "one/one");
        graph.commit().expect("commit");
        drop(graph);

        let graph = reopen(&path);
        let fragment = graph.input(&FRAGMENT, &name).expect("read");
        assert_eq!(fragment.as_deref().map(String::as_str), Some("one"));
        assert!(graph.input(&TRANSIENT, &name).expect("read").is_none());
    }

    #[test]
    fn setting_an_equal_value_after_reopen_keeps_what_read_it_green() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("graph.db");
        let name = "a".to_owned();
        let mut graph = reopen(&path);
        graph
            .set_input(&FRAGMENT, &name, "one".to_owned(), Durability::Volatile)
            .expect("set");
        graph.query(&SEEN, &name).expect("pull");
        graph.commit().expect("commit");
        drop(graph);

        let mut graph = reopen(&path);
        graph
            .set_input(&FRAGMENT, &name, "one".to_owned(), Durability::Volatile)
            .expect("set");
        graph.query(&SEEN, &name).expect("pull");
        assert!(graph.take_executed().is_empty());
        graph
            .set_input(&FRAGMENT, &name, "two".to_owned(), Durability::Volatile)
            .expect("set");
        assert_eq!(*graph.query(&SEEN, &name).expect("pull"), "two/-");
    }
}

mod lazy {
    use lighthouse_graph::{Durability, Graph, Input, Options, Query};

    static BIG: Input<String, String> = Input::new("big").persisted();
    static LEN: Query<String, usize> = Query::new("len", |graph, name| {
        Ok(graph.input(&BIG, name)?.map_or(0, |big| big.len()))
    });
    static SIZE: Query<String, usize> = Query::new("size", |graph, name| {
        graph.query(&LEN, name).map(|len| *len + 1)
    });
    static TOKEN: Query<String, String> = Query::new("token", |graph, name| {
        graph.query(&LEN, name).map(|len| format!("{len}"))
    })
    .transient();
    static HASHED: Query<String, usize> = Query::new("hashed", |graph, name| {
        LEN.hash(graph, name).map(|hash| usize::from(hash[0]))
    });

    fn reopen(path: &std::path::Path) -> Graph {
        let options = Options {
            verify: 0,
            ..Options::default()
        };
        Graph::open(path, options).expect("open")
    }

    #[test]
    fn validating_what_reads_a_persisted_input_does_not_load_it() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("graph.db");
        let name = "a".to_owned();
        let mut graph = reopen(&path);
        graph
            .set_input(&BIG, &name, "four".to_owned(), Durability::Volatile)
            .expect("set");
        assert_eq!(*graph.query(&SIZE, &name).expect("pull"), 5);
        assert_eq!(*graph.query(&TOKEN, &name).expect("pull"), "4");
        graph.commit().expect("commit");
        drop(graph);

        let graph = reopen(&path);
        assert_eq!(*graph.query(&SIZE, &name).expect("pull"), 5);
        assert!(
            graph.take_executed().is_empty(),
            "green without loading or running anything"
        );
        // A transient value is not stored, so it is computed again; what it
        // reads is still green.
        assert_eq!(*graph.query(&TOKEN, &name).expect("pull"), "4");
        assert_eq!(graph.take_executed(), ["token(\"a\")"]);
    }

    #[test]
    fn a_hash_read_moves_with_the_value_and_is_not_a_value_read() {
        let mut graph = Graph::in_memory(Options::default()).expect("open");
        let name = "a".to_owned();
        graph
            .set_input(&BIG, &name, "one".to_owned(), Durability::Volatile)
            .expect("set");
        let first = LEN.hash(&graph, &name).expect("hash");
        graph
            .set_input(&BIG, &name, "three".to_owned(), Durability::Volatile)
            .expect("set");
        assert_ne!(LEN.hash(&graph, &name).expect("hash"), first);
        let after = *graph.query(&HASHED, &name).expect("pull");
        assert_eq!(
            after,
            usize::from(LEN.hash(&graph, &name).expect("hash")[0])
        );
    }
}

mod transient {
    use lighthouse_graph::{Durability, Graph, Input, Options, Query};

    static SEED: Input<(), u8> = Input::new("seed");
    static HELD: Query<(), String> = Query::new("held", |graph, _| {
        Ok("h".repeat(usize::from(
            graph.input(&SEED, &())?.map_or(0, |seed| *seed),
        )))
    })
    .transient();
    static READER: Query<(), usize> =
        Query::new("reader", |graph, _| Ok(graph.query(&HELD, &())?.len()));

    fn open(path: &std::path::Path, seed: u8) -> Graph {
        let mut graph = Graph::open(
            path,
            Options {
                verify: 0,
                ..Options::default()
            },
        )
        .expect("open");
        HELD.register(&graph).expect("register");
        graph
            .set_input(&SEED, &(), seed, Durability::Stable)
            .expect("set");
        graph
    }

    #[test]
    fn a_new_process_finds_what_read_a_transient_query_green() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("graph.db");
        let mut graph = open(&path, 3);
        assert_eq!(*graph.query(&READER, &()).unwrap(), 3);
        graph.commit().expect("commit");
        drop(graph);

        let graph = open(&path, 3);

        assert_eq!(*graph.query(&READER, &()).unwrap(), 3);
        assert_eq!(graph.take_executed(), Vec::<String>::new());
    }

    #[test]
    fn a_new_process_runs_what_read_a_transient_query_when_its_input_moved() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("graph.db");
        let mut graph = open(&path, 3);
        graph.query(&READER, &()).unwrap();
        graph.commit().expect("commit");
        drop(graph);

        let graph = open(&path, 5);

        assert_eq!(*graph.query(&READER, &()).unwrap(), 5);
    }
}

mod file_size {
    use lighthouse_graph::{Durability, Graph, Input, Options, Query};

    static SEED: Input<(), u8> = Input::new("seed");
    static BIG: Query<(), String> = Query::new("big", |graph, _| {
        Ok(format!(
            "{}{}",
            graph.input(&SEED, &())?.unwrap_or_default(),
            "b".repeat(200_000)
        ))
    });

    fn options() -> Options {
        Options {
            verify: 0,
            ..Options::default()
        }
    }

    fn mode(path: &std::path::Path) -> i64 {
        let conn = rusqlite::Connection::open(path).expect("open the file");
        conn.query_row("PRAGMA auto_vacuum", [], |row| row.get(0))
            .expect("mode")
    }

    #[test]
    fn a_file_is_incremental_and_shrinks_when_values_are_trimmed() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("graph.db");
        let mut graph = Graph::open(&path, options()).expect("open");
        graph
            .set_input(&SEED, &(), 1, Durability::Volatile)
            .expect("set");
        graph.query(&BIG, &()).expect("pull");
        graph.commit().expect("commit");
        lighthouse_graph::trim(&mut graph, 0).expect("trim");
        let full = std::fs::metadata(&path).expect("stat").len();

        lighthouse_graph::trim(&mut graph, u64::MAX).expect("trim");

        let small = std::fs::metadata(&path).expect("stat").len();
        assert_eq!(mode(&path), 2);
        assert!(full > 150_000 && small < 50_000, "{full} -> {small}");
    }

    #[test]
    fn trimming_goes_through_retained_states_before_the_values_of_nodes() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("graph.db");
        let mut graph = Graph::open(&path, options()).expect("open");
        for seed in 1..=3 {
            graph
                .set_input(&SEED, &(), seed, Durability::Volatile)
                .expect("set");
            graph.query(&BIG, &()).expect("pull");
            graph.commit().expect("commit");
        }
        graph.take_executed();

        lighthouse_graph::trim(&mut graph, 300_000).expect("trim");

        graph.query(&BIG, &()).expect("pull");
        assert!(graph.take_executed().is_empty(), "the current value stays");
    }

    #[test]
    fn a_file_made_without_incremental_vacuum_is_replaced() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("graph.db");
        drop(Graph::open(&path, options()).expect("open"));
        {
            let conn = rusqlite::Connection::open(&path).expect("open the file");
            conn.pragma_update(None, "auto_vacuum", 0).expect("set");
            conn.execute_batch("VACUUM").expect("vacuum");
            assert_eq!(mode(&path), 0);
        }

        drop(Graph::open(&path, options()).expect("open"));

        assert_eq!(mode(&path), 2);
    }
}

mod inputs_stay {
    use lighthouse_graph::{Durability, Graph, Input, Options, Query};

    static KEPT: Input<(), String> = Input::new("kept");
    static SAME: Query<(), String> = Query::new("same", |graph, _| {
        Ok(graph
            .input(&KEPT, &())?
            .map_or_else(String::new, |v| v.as_ref().clone()))
    });

    #[test]
    fn trimming_never_drops_an_input_or_a_value_it_shares() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("graph.db");
        let options = Options {
            verify: 0,
            ..Options::default()
        };
        let mut graph = Graph::open(&path, options).expect("open");
        graph
            .set_input(&KEPT, &(), "k".repeat(50_000), Durability::Volatile)
            .expect("set");
        // A query whose value is the very bytes of the input.
        graph.query(&SAME, &()).expect("pull");
        graph.commit().expect("commit");

        lighthouse_graph::trim(&mut graph, u64::MAX).expect("trim");
        drop(graph);

        let conn = rusqlite::Connection::open(&path).expect("open the file");
        let held: i64 = conn
            .query_row(
                "SELECT count(*) FROM node JOIN blob ON blob.hash = node.hash WHERE node.kind = 'kept'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(held, 1);
    }
}

mod holds {
    use lighthouse_graph::{Durability, Graph, Input, Options};

    static SEED: Input<(), u8> = Input::new("seed");

    #[test]
    fn holds_tells_a_set_input_from_one_never_set_without_reading_it() {
        let mut graph = Graph::in_memory(Options {
            verify: 0,
            ..Options::default()
        })
        .expect("open");

        assert!(!lighthouse_graph::holds(&graph, &SEED, &()).expect("holds"));

        graph
            .set_input(&SEED, &(), 1, Durability::Stable)
            .expect("set");

        assert!(lighthouse_graph::holds(&graph, &SEED, &()).expect("holds"));
    }
}
