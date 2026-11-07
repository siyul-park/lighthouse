//! An edit and its revert: what the nodes retain is taken back, and nothing
//! runs.

use lighthouse_graph::{Durability, Input, Options, RETAINED, Reason};

use crate::{differential::*, toy::*};

static ANSWER: Input<String, String> = Input::new("answer").persisted();

fn project() -> Project {
    let mut project = Project::new();
    for (path, text) in [
        ("a", "api=A1;body=a0;uses=b"),
        ("b", "api=B1;body=b0;uses=c"),
        ("c", "api=C1;body=c0;uses="),
    ] {
        project.files.insert(path.to_owned(), text.to_owned());
    }
    project
}

fn known(project: &Project) -> Vec<String> {
    project.files.keys().cloned().collect()
}

fn edited(project: &Project, path: &str, text: &str) -> Project {
    let mut project = project.clone();
    project.files.insert(path.to_owned(), text.to_owned());
    project
}

#[test]
fn a_revert_takes_the_earlier_values_back_and_runs_nothing() {
    let before = project();
    let mut graph = open(None, &before);
    apply(&mut graph, &before, &known(&before));
    let first = pull(&graph);
    let after = edited(&before, "c", "api=C2;body=c1;uses=");
    apply(&mut graph, &after, &known(&after));
    assert_ne!(pull(&graph), first);
    graph.take_executed();

    apply(&mut graph, &before, &known(&before));

    assert_eq!(pull(&graph), first);
    let ran = graph.take_executed();
    assert!(ran.is_empty(), "ran {ran:?}");
}

#[test]
fn a_node_that_was_not_pulled_at_the_edit_is_green_at_the_revert() {
    let before = project();
    let mut graph = open(None, &before);
    apply(&mut graph, &before, &known(&before));
    let first = pull(&graph);
    graph.take_executed();
    let after = edited(&before, "c", "api=C2;body=c1;uses=");
    apply(&mut graph, &after, &known(&after));
    apply(&mut graph, &before, &known(&before));

    assert_eq!(pull(&graph), first);

    assert!(graph.take_executed().is_empty());
}

#[test]
fn a_node_keeps_the_last_retained_values_and_no_more() {
    let mut project = project();
    let mut graph = open(None, &project);
    apply(&mut graph, &project, &known(&project));
    let first = pull(&graph);
    for n in 0..=RETAINED {
        project = edited(&project, "c", &format!("api=C1;body=c{};uses=", n + 1));
        apply(&mut graph, &project, &known(&project));
        pull(&graph);
    }
    graph.take_executed();

    let back = edited(&project, "c", "api=C1;body=c0;uses=");
    apply(&mut graph, &back, &known(&back));

    assert_eq!(pull(&graph), first);
    assert!(!graph.take_executed().is_empty(), "too old to be kept");
}

#[test]
fn a_revert_in_a_new_process_runs_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("graph.db");
    let before = project();
    let mut graph = open(Some(&path), &before);
    apply(&mut graph, &before, &known(&before));
    let first = pull(&graph);
    let after = edited(&before, "c", "api=C2;body=c1;uses=");
    apply(&mut graph, &after, &known(&after));
    pull(&graph);
    graph.commit().expect("commit");
    drop(graph);

    let mut graph = open(Some(&path), &before);
    apply(&mut graph, &before, &known(&before));

    assert_eq!(pull(&graph), first);
    assert!(graph.take_executed().is_empty());
}

#[test]
fn a_retained_value_is_explained_as_taken_back() {
    let before = project();
    let mut graph = open(None, &before);
    apply(&mut graph, &before, &known(&before));
    pull(&graph);
    let after = edited(&before, "c", "api=C1;body=c1;uses=");
    apply(&mut graph, &after, &known(&after));
    pull(&graph);
    apply(&mut graph, &before, &known(&before));
    pull(&graph);

    let why = graph
        .explain(&FACTS, &"c".to_owned())
        .expect("explain")
        .expect("touched");

    assert_eq!(why.0[0].reason, Reason::Retained);
}

#[test]
fn the_earlier_values_of_a_persisted_input_are_read_newest_first_after_a_reopen() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("graph.db");
    let name = "a".to_owned();
    let mut graph = open_with(Some(&path), Options::default());
    for value in ["one", "two", "three"] {
        graph
            .set_input(&ANSWER, &name, value.to_owned(), Durability::Volatile)
            .expect("set");
    }
    graph.commit().expect("commit");
    drop(graph);

    let mut graph = open_with(Some(&path), Options::default());
    graph
        .set_input(&ANSWER, &name, "four".to_owned(), Durability::Volatile)
        .expect("set");

    let earlier = ANSWER.retained(&graph, &name).expect("retained");

    let earlier: Vec<&str> = earlier.iter().map(|value| value.as_str()).collect();
    assert_eq!(earlier, ["three", "two", "one"]);
}

#[test]
fn digest() {
    let (one, two) = ("one".to_owned(), "two".to_owned());

    let (first, again, other) = (
        lighthouse_graph::digest(&one).expect("digest"),
        lighthouse_graph::digest(&one.clone()).expect("digest"),
        lighthouse_graph::digest(&two).expect("digest"),
    );

    assert_eq!(first, again);
    assert_ne!(first, other);
}

/// Edits one file of a random project and puts it back: the report is the
/// one before, and no node ran.
#[test]
fn a_revert_after_random_edits_equals_from_scratch_and_runs_nothing() {
    for seed in 1..=40 {
        let mut rng = Rng::new(seed);
        let mut sim = Sim::new();
        let verifying = Options {
            verify: 1,
            ..options(&sim.project)
        };
        let mut graph = open_with(None, verifying);
        for _ in 0..20 {
            sim.step(&mut rng);
        }
        apply(&mut graph, &sim.project, &sim.known);
        let first = pull(&graph);
        let paths: Vec<String> = sim.project.files.keys().cloned().collect();
        let path = paths[rng.below(paths.len())].clone();
        let before = sim.project.clone();
        let text = format!("api=A{seed};body=edit{seed};uses=");
        sim.project.files.insert(path, text);
        apply(&mut graph, &sim.project, &sim.known);
        pull(&graph);
        graph.take_executed();

        apply(&mut graph, &before, &sim.known);

        assert_eq!(pull(&graph), first, "seed {seed}");
        assert_eq!(first, from_scratch(&before, &sim.known), "seed {seed}");
        let ran = graph.take_executed();
        assert!(ran.is_empty(), "seed {seed} ran {ran:?}");
    }
}
