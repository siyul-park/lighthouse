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
    let mut db = open(Some(&path), &sim.project);
    db.set_verify(0);
    apply(&mut db, &sim.project, &sim.known);
    pull(&db);
    db.commit().expect("commit");
    db.take_executed();
    let mut trace = Vec::new();
    for step in 1..=steps {
        let edit = sim.step(&mut rng);
        if reopen != 0 && step % reopen == 0 {
            drop(db);
            db = open(Some(&path), &sim.project);
            db.set_verify(0);
        }
        if matches!(edit, Edit::Evict) {
            for file in sim.project.files.keys() {
                db.evict(&FACTS, file).expect("evict");
            }
        }
        apply(&mut db, &sim.project, &sim.known);
        let report = pull(&db);
        let mut ran = db.take_executed();
        ran.sort();
        db.commit().expect("commit");
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
    let mut db = open(Some(&path), &sim.project);
    apply(&mut db, &sim.project, &sim.known);
    let first = pull(&db);
    db.commit().expect("commit");
    drop(db);

    let mut db = open(Some(&path), &sim.project);
    apply(&mut db, &sim.project, &sim.known);

    assert_eq!(pull(&db), first);
    assert!(db.take_executed().is_empty());
}

#[test]
fn a_changed_version_runs_its_readers_in_a_new_process() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("graph.db");
    let sim = Sim::new();
    let mut db = open(Some(&path), &sim.project);
    apply(&mut db, &sim.project, &sim.known);
    pull(&db);
    db.commit().expect("commit");
    drop(db);
    let mut project = sim.project.clone();
    project.versions.insert("parser".into(), "p2".into());

    let mut db = open(Some(&path), &project);
    apply(&mut db, &project, &sim.known);
    pull(&db);

    let ran = db.take_executed();
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

    let mut db = open(Some(&path), &sim.project);
    apply(&mut db, &sim.project, &sim.known);

    assert_eq!(pull(&db), from_scratch(&sim.project, &sim.known));
}

#[test]
fn the_byte_limit_drops_values_and_keeps_results() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("graph.db");
    let sim = Sim::new();
    let mut db = open(Some(&path), &sim.project);
    db.set_verify(0);
    db.set_limit(1);
    apply(&mut db, &sim.project, &sim.known);
    let first = pull(&db);
    db.commit().expect("commit");

    assert_eq!(pull(&db), first);
    db.take_executed();
    assert_eq!(pull(&db), first);
    assert!(db.take_executed().is_empty());
}

mod eviction_order {
    use lighthouse_graph::{Db, Durability, Error, Input, Query};

    static SEED: Input<(), u8> = Input::new("seed");
    static STATIC_BIG: Query<(), String> = Query::new("static-big", |db, _| {
        Ok(format!("{}{}", db.version("v")?, "s".repeat(1000)))
    });
    static VOLATILE_BIG: Query<(), String> = Query::new("volatile-big", |db, _| {
        Ok(format!("{}{}", db.input(&SEED, &())?, "v".repeat(1000)))
    });

    fn pull(db: &Db) -> Result<(), Error> {
        db.query(&STATIC_BIG, &())?;
        db.query(&VOLATILE_BIG, &())?;
        Ok(())
    }

    #[test]
    fn volatile_values_go_before_static_ones() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("graph.db");
        let versions = [("v".to_owned(), "1".to_owned())].into();
        let mut db = Db::open(&path, &versions).expect("open");
        db.set_verify(0);
        db.register_query(&STATIC_BIG);
        db.register_query(&VOLATILE_BIG);
        db.set_input(&SEED, &(), 1, Durability::Volatile)
            .expect("set");
        db.set_limit(1500);
        pull(&db).expect("pull");
        db.commit().expect("commit");
        db.take_executed();

        drop(db);
        let mut db = Db::open(&path, &versions).expect("open");
        db.set_verify(0);
        db.register_query(&STATIC_BIG);
        db.register_query(&VOLATILE_BIG);
        db.set_input(&SEED, &(), 1, Durability::Volatile)
            .expect("set");
        pull(&db).expect("pull");

        assert_eq!(db.take_executed(), ["volatile-big(())"]);
    }
}

#[test]
fn a_state_blob_is_read_back_after_commit_and_reopen() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("graph.db");
    let project = Project::new();
    let mut db = open(Some(&path), &project);
    assert_eq!(db.state("walk").expect("state"), None);

    db.set_state("walk", b"one".to_vec());
    assert_eq!(db.state("walk").expect("state"), Some(b"one".to_vec()));
    drop(db);
    let db = open(Some(&path), &project);
    assert_eq!(db.state("walk").expect("state"), None, "never committed");

    let mut db = db;
    db.set_state("walk", b"two".to_vec());
    db.commit().expect("commit");
    drop(db);
    let db = open(Some(&path), &project);
    assert_eq!(db.state("walk").expect("state"), Some(b"two".to_vec()));
}
