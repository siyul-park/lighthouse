//! The behaviours the engine relies on, one at a time.

use std::collections::BTreeSet;

use lighthouse_graph::{Durability, Error, Graph, Input, Options, Query, Reason, Revision};

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

fn executed(graph: &Graph) -> BTreeSet<String> {
    graph.take_executed().into_iter().collect()
}

fn set(of: &[&str]) -> BTreeSet<String> {
    of.iter().map(|label| (*label).to_owned()).collect()
}

fn graph(project: &Project) -> Graph {
    let mut graph = open(None, project);
    apply(&mut graph, project, &known(project));
    pull(&graph);
    graph.take_executed();
    graph
}

#[test]
fn setting_an_equal_input_runs_nothing() {
    let project = project();
    let mut graph = graph(&project);

    apply(&mut graph, &project, &known(&project));

    pull(&graph);
    assert!(graph.take_executed().is_empty());
}

#[test]
fn a_body_edit_runs_that_file_and_backdates_its_api() {
    let mut project = project();
    let mut graph = graph(&project);
    project
        .files
        .insert("b".into(), "api=B1;body=b1;uses=c".into());

    apply(&mut graph, &project, &known(&project));
    pull(&graph);

    assert_eq!(
        executed(&graph),
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
    let mut graph = graph(&project);
    project
        .files
        .insert("b".into(), "api=B2;body=b0;uses=c".into());

    apply(&mut graph, &project, &known(&project));
    pull(&graph);

    let ran = executed(&graph);
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
    let mut graph = graph(&project);
    project.versions.insert("facts".into(), "f2".into());

    apply(&mut graph, &project, &known(&project));
    pull(&graph);

    let ran = executed(&graph);
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
    let mut graph = graph(&project);
    project
        .files
        .insert("d".into(), "api=D1;body=d1;uses=".into());

    apply(&mut graph, &project, &known(&project));
    pull(&graph);

    assert_eq!(
        executed(&graph),
        set(&["parse(\"d\")", "uses(\"d\")", "facts(\"d\")", "report(())"])
    );
}

#[test]
fn an_evicted_node_is_run_again_only_when_pulled() {
    let project = project();
    let mut graph = graph(&project);
    graph.evict(&FACTS, &"c".to_owned()).expect("evict");
    graph.evict(&API, &"c".to_owned()).expect("evict");

    graph.query(&REPORT, &()).expect("report");

    assert_eq!(executed(&graph), set(&[]));

    graph.evict(&REPORT, &()).expect("evict");
    graph.query(&REPORT, &()).expect("report");

    assert_eq!(executed(&graph), set(&["report(())", "facts(\"c\")"]));
}

#[test]
fn explain_walks_down_to_the_input() {
    let mut project = project();
    let mut graph = graph(&project);
    project
        .files
        .insert("b".into(), "api=B2;body=b0;uses=c".into());
    apply(&mut graph, &project, &known(&project));
    pull(&graph);

    let why = graph
        .explain(&REPORT, &())
        .expect("explain")
        .expect("known");

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

    use lighthouse_graph::{Graph, Query};

    static OUTSIDE: AtomicU64 = AtomicU64::new(0);
    static IMPURE: Query<(), u64> = Query::new("impure", |_: &Graph, _: &()| {
        Ok(OUTSIDE.load(Ordering::SeqCst))
    });
    let options = Options {
        verify: 1,
        ..Options::default()
    };
    let mut graph = Graph::in_memory(options).expect("open");
    assert_eq!(*graph.query(&IMPURE, &()).expect("first"), 0);
    OUTSIDE.store(1, Ordering::SeqCst);
    graph
        .set_input(&FILES, &(), vec![], Durability::Volatile)
        .expect("set");

    let pulled = graph.query(&IMPURE, &());

    assert!(matches!(pulled, Err(Error::Impure(_))), "{pulled:?}");
}

static MAYBE: Input<u32, u32> = Input::new("maybe");
static READS_MAYBE: Query<u32, u32> = Query::new("reads-maybe", |graph, key| {
    Ok(graph.input(&MAYBE, key)?.map_or(0, |value| *value + 1))
});

fn in_memory() -> Graph {
    let options = Options {
        verify: 0,
        ..Options::default()
    };
    Graph::in_memory(options).expect("open")
}

#[test]
fn a_query_that_found_an_input_unset_runs_again_when_it_is_set() {
    let mut graph = in_memory();
    assert_eq!(*graph.query(&READS_MAYBE, &1).expect("pull"), 0);

    graph
        .set_input(&MAYBE, &1, 41, Durability::Volatile)
        .expect("set");
    assert_eq!(*graph.query(&READS_MAYBE, &1).expect("pull"), 42);

    graph
        .set_input(&MAYBE, &1, 41, Durability::Volatile)
        .expect("set");
    graph.take_executed();
    assert_eq!(*graph.query(&READS_MAYBE, &1).expect("pull"), 42);
    assert!(
        graph.take_executed().is_empty(),
        "an equal input moves nothing"
    );
}

#[test]
fn a_query_that_found_an_input_unset_stays_green_while_it_is_unset() {
    let mut graph = in_memory();
    graph
        .set_input(&FILE, &"other".to_owned(), None, Durability::Volatile)
        .expect("set");
    assert_eq!(*graph.query(&READS_MAYBE, &1).expect("pull"), 0);
    graph.take_executed();

    graph
        .set_input(
            &FILE,
            &"other".to_owned(),
            Some("x".into()),
            Durability::Volatile,
        )
        .expect("set");

    assert_eq!(*graph.query(&READS_MAYBE, &1).expect("pull"), 0);
    assert!(graph.take_executed().is_empty(), "nothing it read moved");
}

/// An explanation has a line for each step from the node to the input.
#[test]
fn explanation() {
    let mut project = Project::new();
    project
        .files
        .insert("a".into(), "api=A1;body=B1;uses=".into());
    let mut graph = open(None, &project);
    apply(&mut graph, &project, &["a".to_owned()]);
    pull(&graph);
    project
        .files
        .insert("a".into(), "api=A1;body=B2;uses=".into());
    apply(&mut graph, &project, &["a".to_owned()]);
    pull(&graph);

    let explanation = graph
        .explain(&FACTS, &"a".to_owned())
        .expect("explain")
        .expect("it ran");

    let lines = explanation.to_string();
    assert_eq!(lines.lines().count(), explanation.0.len());
    assert!(lines.contains("file"), "{lines}");
}

/// A revision prints as `r` and its number.
#[test]
fn revision() {
    assert_eq!(Revision(3).to_string(), "r3");
}

static FILE_TEXT: Input<String, String> = Input::new("file");
static FILE_FLAG: Input<String, bool> = Input::new("file");
static FILE_QUERY: Query<String, Option<String>> = Query::new("file", |_, _| Ok(None));

/// A name is global: another key or value type, or an input registered as a
/// query, is an error that names both.
#[test]
fn a_kind_name_registered_with_another_type_is_an_error() {
    let mut graph = Graph::in_memory(Options::default()).expect("open");
    graph
        .set_input(
            &FILE,
            &"a".to_owned(),
            Some("x".to_owned()),
            Durability::Volatile,
        )
        .expect("first registration");

    let value = graph.set_input(&FILE_FLAG, &"a".to_owned(), true, Durability::Volatile);
    let key = graph.input(&FILE_TEXT, &"a".to_owned());
    let kind = graph.query(&FILE_QUERY, &"a".to_owned());

    for found in [value.err(), key.err(), kind.err()] {
        let Some(Error::Kind { name, had, now }) = found else {
            panic!("expected a kind conflict, got {found:?}");
        };
        assert_eq!(name, "file");
        assert!(had.contains("Option<alloc::string::String>"), "{had}");
        assert_ne!(had, now);
    }
    assert!(
        graph.input(&FILE, &"a".to_owned()).is_ok(),
        "the first stays"
    );
}
