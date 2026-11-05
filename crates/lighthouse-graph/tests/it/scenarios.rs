//! The behaviours the engine relies on, one at a time.

use std::collections::BTreeSet;

use lighthouse_graph::{Durability, Error, Input, Query, Reason};

use crate::toy::*;

fn project() -> Project {
    let mut project = Project::new();
    for (path, text) in [
        ("a", "api=A1;body=a0;uses=b"),
        ("b", "api=B1;body=b0;uses=c"),
        ("c", "api=C1;body=c0;uses="),
        ("d", "api=D1;body=d0;uses="),
    ] {
        project.files.insert(path.to_owned(), text.to_owned());
    }
    project
}

fn known(project: &Project) -> Vec<String> {
    project.files.keys().cloned().collect()
}

fn executed(db: &lighthouse_graph::Db) -> BTreeSet<String> {
    db.take_executed().into_iter().collect()
}

fn set(of: &[&str]) -> BTreeSet<String> {
    of.iter().map(|label| (*label).to_owned()).collect()
}

fn graph(project: &Project) -> lighthouse_graph::Db {
    let mut db = open(None, project);
    db.set_verify(0);
    apply(&mut db, project, &known(project));
    pull(&db);
    db.take_executed();
    db
}

#[test]
fn setting_an_equal_input_moves_no_revision() {
    let project = project();
    let mut db = graph(&project);
    let before = db.revision();

    apply(&mut db, &project, &known(&project));

    assert_eq!(db.revision(), before);
    pull(&db);
    assert!(db.take_executed().is_empty());
}

#[test]
fn a_body_edit_runs_that_file_and_backdates_its_api() {
    let mut project = project();
    let mut db = graph(&project);
    project
        .files
        .insert("b".into(), "api=B1;body=b1;uses=c".into());

    apply(&mut db, &project, &known(&project));
    pull(&db);

    assert_eq!(
        executed(&db),
        set(&[
            "parse(\"b\")",
            "api(\"b\")",
            "uses(\"b\")",
            "facts(\"b\")",
            "report(())",
        ])
    );
}

#[test]
fn an_api_edit_reaches_the_files_that_use_it_and_no_further() {
    let mut project = project();
    let mut db = graph(&project);
    project
        .files
        .insert("b".into(), "api=B2;body=b0;uses=c".into());

    apply(&mut db, &project, &known(&project));
    pull(&db);

    let ran = executed(&db);
    assert!(ran.contains("deps(\"a\")") && ran.contains("facts(\"a\")"));
    assert!(
        !ran.iter()
            .any(|label| path_of(label) == "c" || path_of(label) == "d")
    );
    assert!(!ran.contains("api(\"a\")") && !ran.contains("uses(\"a\")"));
}

#[test]
fn a_static_version_invalidates_exactly_its_readers() {
    let mut project = project();
    let mut db = graph(&project);
    project.versions.insert("facts".into(), "f2".into());

    apply(&mut db, &project, &known(&project));
    pull(&db);

    let ran = executed(&db);
    assert!(
        ran.iter()
            .all(|label| matches!(kind_of(label), "facts" | "report")),
        "{ran:?}"
    );
    assert_eq!(
        ran.iter().filter(|label| kind_of(label) == "facts").count(),
        4
    );
}

#[test]
fn a_body_edit_of_an_unused_file_runs_only_that_file() {
    let mut project = project();
    let mut db = graph(&project);
    project
        .files
        .insert("d".into(), "api=D1;body=d1;uses=".into());

    apply(&mut db, &project, &known(&project));
    pull(&db);

    assert_eq!(
        executed(&db),
        set(&["parse(\"d\")", "uses(\"d\")", "facts(\"d\")", "report(())"])
    );
}

#[test]
fn an_evicted_node_is_run_again_only_when_pulled() {
    let project = project();
    let mut db = graph(&project);
    db.evict(&FACTS, &"c".to_owned()).expect("evict");
    db.evict(&API, &"c".to_owned()).expect("evict");

    pull(&db);

    assert_eq!(executed(&db), set(&[]));

    db.evict(&REPORT, &()).expect("evict");
    pull(&db);

    assert_eq!(executed(&db), set(&["report(())", "facts(\"c\")"]));
}

#[test]
fn explain_walks_down_to_the_input() {
    let mut project = project();
    let mut db = graph(&project);
    project
        .files
        .insert("b".into(), "api=B2;body=b0;uses=c".into());
    apply(&mut db, &project, &known(&project));
    pull(&db);

    let why = db.explain(&REPORT, &()).expect("explain").expect("known");

    let nodes: Vec<&str> = why.0.iter().map(|step| step.node.as_str()).collect();
    assert_eq!(nodes.first(), Some(&"report(())"));
    assert_eq!(nodes.last(), Some(&"file(\"b\")"));
    assert!(matches!(
        why.0.last().map(|step| &step.reason),
        Some(Reason::InputChanged(_))
    ));
}

#[test]
fn verify_mode_reports_a_query_that_reads_outside_the_graph() {
    use std::sync::atomic::{AtomicU64, Ordering};

    use lighthouse_graph::{Db, Query};

    static OUTSIDE: AtomicU64 = AtomicU64::new(0);
    static IMPURE: Query<(), u64> =
        Query::new(
            "impure",
            |_: &Db, _: &()| Ok(OUTSIDE.load(Ordering::SeqCst)),
        );
    let mut db = Db::in_memory(&Default::default()).expect("open");
    db.set_verify(1);
    assert_eq!(*db.query(&IMPURE, &()).expect("first"), 0);
    OUTSIDE.store(1, Ordering::SeqCst);
    db.set_input(&FILES, &(), vec![], Durability::Volatile)
        .expect("set");

    let pulled = db.query(&IMPURE, &());

    assert!(matches!(pulled, Err(Error::Impure(_))), "{pulled:?}");
}

static MAYBE: Input<u32, u32> = Input::new("maybe");
static READS_MAYBE: Query<u32, u32> = Query::new("reads-maybe", |db, key| {
    Ok(db.try_input(&MAYBE, key)?.map_or(0, |value| *value + 1))
});

#[test]
fn a_query_that_found_an_input_unset_runs_again_when_it_is_set() {
    let mut db = lighthouse_graph::Db::in_memory(&Default::default()).expect("open");
    db.set_verify(0);
    db.register_input(&MAYBE);
    db.register_query(&READS_MAYBE);
    assert_eq!(*db.query(&READS_MAYBE, &1).expect("pull"), 0);

    db.set_input(&MAYBE, &1, 41, lighthouse_graph::Durability::Volatile)
        .expect("set");
    assert_eq!(*db.query(&READS_MAYBE, &1).expect("pull"), 42);

    db.set_input(&MAYBE, &1, 41, lighthouse_graph::Durability::Volatile)
        .expect("set");
    db.take_executed();
    assert_eq!(*db.query(&READS_MAYBE, &1).expect("pull"), 42);
    assert!(
        db.take_executed().is_empty(),
        "an equal input moves nothing"
    );
}

#[test]
fn a_query_that_found_an_input_unset_stays_green_while_it_is_unset() {
    let mut db = lighthouse_graph::Db::in_memory(&Default::default()).expect("open");
    db.set_verify(0);
    db.register_input(&MAYBE);
    db.register_input(&FILE);
    db.register_query(&READS_MAYBE);
    db.set_input(&FILE, &"other".to_owned(), None, Durability::Volatile)
        .expect("set");
    assert_eq!(*db.query(&READS_MAYBE, &1).expect("pull"), 0);
    db.take_executed();

    db.set_input(
        &FILE,
        &"other".to_owned(),
        Some("x".into()),
        Durability::Volatile,
    )
    .expect("set");

    assert_eq!(*db.query(&READS_MAYBE, &1).expect("pull"), 0);
    assert!(db.take_executed().is_empty(), "nothing it read moved");
}
