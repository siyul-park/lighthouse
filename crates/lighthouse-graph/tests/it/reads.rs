//! What a node read, as the graph recorded it, and which of those moved
//! between two revisions.

use lighthouse_graph::Graph;

use crate::toy::*;

fn project() -> Project {
    let mut project = Project::new();
    for (path, text) in [
        ("a", "api=A1;body=a0;uses=b"),
        ("b", "api=B1;body=b0;uses="),
        ("c", "api=C1;body=c0;uses="),
    ] {
        project.files.insert(path.to_owned(), text.to_owned());
    }
    project
}

fn known(project: &Project) -> Vec<String> {
    project.files.keys().cloned().collect()
}

/// The files `facts` of `a` read, or moved to read, between two revisions.
fn files_of(
    graph: &Graph,
    moved: Option<(lighthouse_graph::Revision, lighthouse_graph::Revision)>,
) -> Vec<String> {
    let reads = graph
        .reads(&FACTS, &"a".to_owned(), (moved, &[]))
        .expect("reads")
        .expect("a node");
    let mut files = reads.keys::<String>(FILE.name());
    files.sort();
    files
}

fn edited(project: &Project, path: &str, text: &str) -> Project {
    let mut project = project.clone();
    project.files.insert(path.to_owned(), text.to_owned());
    project
}

#[test]
fn a_node_reads_the_files_it_was_made_of_and_no_other() {
    let project = project();
    let mut graph = open(None, &project);
    apply(&mut graph, &project, &known(&project));
    pull(&graph);

    assert_eq!(files_of(&graph, None), ["a", "b"]);
}

#[test]
fn a_node_moved_by_the_files_that_changed_between_two_revisions_and_no_other() {
    let before = project();
    let mut graph = open(None, &before);
    apply(&mut graph, &before, &known(&before));
    pull(&graph);
    let since = graph.revision();

    let after = edited(
        &edited(&before, "b", "api=B2;body=b0;uses="),
        "c",
        "api=C2;body=c0;uses=",
    );
    apply(&mut graph, &after, &known(&after));
    pull(&graph);
    let until = graph.revision();

    assert_eq!(files_of(&graph, Some((since, until))), ["b"]);
}

#[test]
fn a_node_the_graph_does_not_hold_has_no_reads() {
    let project = project();
    let graph = open(None, &project);

    let reads = graph
        .reads(&FACTS, &"a".to_owned(), (None, &[]))
        .expect("reads");

    assert!(reads.is_none());
}
