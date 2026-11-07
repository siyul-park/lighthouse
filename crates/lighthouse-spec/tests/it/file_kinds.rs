//! The file kinds of the graph, pulled through `ProjectFiles::walk`.

use std::{collections::BTreeMap, fs, path::Path};

use lighthouse_graph::{Durability, Graph, Options};
use lighthouse_protocol::hash;
use lighthouse_spec::{CONTENT, Content, Held, OVERLAY, ProjectFiles, ROOT, Readable, STAT};

fn graph() -> Graph {
    Graph::in_memory(Options::default()).unwrap()
}

/// A graph that holds `dir` as the root and the `stat` of what is under it.
fn graph_over(dir: &Path, files: &[&str]) -> Graph {
    let mut graph = graph();
    let root = lighthouse_spec::key(dir);
    graph
        .set_input(&ROOT, &(), root, Durability::Static)
        .unwrap();
    for file in files {
        let stat = lighthouse_spec::Stat::of(&fs::symlink_metadata(dir.join(file)).unwrap());
        graph
            .set_input(
                &STAT,
                &(*file).to_owned(),
                Some((stat, None)),
                Durability::Volatile,
            )
            .unwrap();
    }
    graph
}

#[test]
fn content() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "hello").unwrap();
    fs::write(dir.path().join("b.bin"), [0xff, 0xfe, 0x00]).unwrap();
    let mut graph = graph_over(dir.path(), &["a.txt", "b.bin"]);

    let text = graph.query(&CONTENT, &"a.txt".to_owned()).unwrap();
    let binary = graph.query(&CONTENT, &"b.bin".to_owned()).unwrap();
    let gone = graph.query(&CONTENT, &"gone.txt".to_owned()).unwrap();
    graph
        .set_input(
            &OVERLAY,
            &"a.txt".to_owned(),
            Some(Content::of_text("edited")),
            Durability::Volatile,
        )
        .unwrap();
    let overlaid = graph.query(&CONTENT, &"a.txt".to_owned()).unwrap();

    assert_eq!(text.readable, Readable::Text);
    assert_eq!(text.sha256, hash::sha256("hello"));
    assert_eq!(binary.readable, Readable::NotUtf8);
    assert_eq!(gone.readable, Readable::Missing);
    assert_eq!(overlaid.text.as_deref(), Some("edited"));
}

#[test]
fn content_of_text() {
    let content = Content::of_text("hello");

    assert_eq!(content.readable, Readable::Text);
    assert_eq!(content.text.as_deref(), Some("hello"));
    assert_eq!(content.sha256, hash::sha256("hello"));
    assert_eq!(content.blake3, *blake3::hash(b"hello").as_bytes());
}

#[test]
fn held() {
    let held = Held::default();

    assert!(held.overlaid().is_empty());
}

#[test]
fn held_overlaid() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "disk").unwrap();
    let project = ProjectFiles::new(dir.path(), &[]).unwrap();
    let (mut graph, mut held) = (graph(), Held::default());
    let overlay = |text: &str| BTreeMap::from([("a.txt".into(), text.to_owned())]);

    project
        .walk(&mut graph, &mut held, &overlay("edit"))
        .unwrap();
    let during = held.overlaid().clone();
    let shown = graph.query(&CONTENT, &"a.txt".to_owned()).unwrap();
    project
        .walk(&mut graph, &mut held, &BTreeMap::new())
        .unwrap();
    let after = graph.query(&CONTENT, &"a.txt".to_owned()).unwrap();

    assert_eq!(during.iter().collect::<Vec<_>>(), ["a.txt"]);
    assert_eq!(shown.text.as_deref(), Some("edit"));
    assert!(held.overlaid().is_empty());
    assert_eq!(after.sha256, hash::sha256("disk"), "the overlay is unset");
}

#[test]
fn key() {
    assert_eq!(lighthouse_spec::key(Path::new("a/b.txt")), "a/b.txt");
    assert_eq!(lighthouse_spec::key(Path::new("")), "");
}

#[test]
fn text() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "hello").unwrap();
    fs::write(dir.path().join("b.bin"), [0xff, 0xfe, 0x00]).unwrap();
    let mut graph = graph_over(dir.path(), &["a.txt", "b.bin"]);

    let read = lighthouse_spec::text(&graph, "a.txt").unwrap();
    let binary = lighthouse_spec::text(&graph, "b.bin").unwrap();
    graph
        .set_input(
            &OVERLAY,
            &"a.txt".to_owned(),
            Some(Content::of_text("edited")),
            Durability::Volatile,
        )
        .unwrap();
    let overlaid = lighthouse_spec::text(&graph, "a.txt").unwrap();

    assert_eq!(read.as_deref(), Some("hello"));
    assert_eq!(binary, None);
    assert_eq!(overlaid.as_deref(), Some("edited"));
}

#[test]
fn held_set_overlays() {
    let (mut graph, mut held) = (graph(), Held::default());
    let overlay = BTreeMap::from([("a.txt".into(), "edit".to_owned())]);

    held.set_overlays(&mut graph, &overlay).unwrap();
    let shown = graph.query(&CONTENT, &"a.txt".to_owned()).unwrap();
    held.set_overlays(&mut graph, &BTreeMap::new()).unwrap();

    assert_eq!(shown.text.as_deref(), Some("edit"));
    assert!(held.overlaid().is_empty());
}
