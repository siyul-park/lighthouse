//! A query that its first dependencies decide is current while they have not
//! moved, whatever the rest of what it read has done.

use lighthouse_graph::{Durability, Error, Graph, Input, Options, Query};

static STATE: Input<(), u32> = Input::new("state");
static DETAIL: Input<(), u32> = Input::new("detail");
static SUMMARY: Query<(), u32> = Query::new("summary", summary).decided_by(1);

fn summary(graph: &Graph, _: &()) -> Result<u32, Error> {
    let state = graph.input(&STATE, &())?.map_or(0, |v| *v);
    let detail = graph.input(&DETAIL, &())?.map_or(0, |v| *v);
    Ok(state * 100 + detail)
}

fn set(graph: &mut Graph, state: u32, detail: u32) {
    graph
        .set_input(&STATE, &(), state, Durability::Volatile)
        .expect("state");
    graph
        .set_input(&DETAIL, &(), detail, Durability::Volatile)
        .expect("detail");
}

#[test]
fn the_dependencies_that_decide_stand_for_the_rest() {
    // The verify mode runs green nodes again to find one that is not pure,
    // and a node whose rest is not looked at is not, by what it declares.
    let options = Options {
        verify: 0,
        ..Options::default()
    };
    let mut graph = Graph::in_memory(options).expect("graph");
    set(&mut graph, 1, 1);
    assert_eq!(*graph.query(&SUMMARY, &()).expect("pull"), 101);
    graph.take_executed();

    set(&mut graph, 1, 2);
    assert_eq!(*graph.query(&SUMMARY, &()).expect("pull"), 101);
    assert!(
        graph.take_executed().is_empty(),
        "the detail is not looked at"
    );

    set(&mut graph, 2, 2);
    assert_eq!(*graph.query(&SUMMARY, &()).expect("pull"), 202);
    assert_eq!(graph.take_executed(), ["summary(())"]);
}

#[test]
fn a_query_that_was_never_computed_is_not_fresh() {
    let mut graph = Graph::in_memory(Options::default()).expect("graph");
    set(&mut graph, 1, 1);
    assert!(!graph.is_fresh(&SUMMARY, &()).expect("fresh"));
    graph.query(&SUMMARY, &()).expect("pull");
    assert!(graph.is_fresh(&SUMMARY, &()).expect("fresh"));
}
