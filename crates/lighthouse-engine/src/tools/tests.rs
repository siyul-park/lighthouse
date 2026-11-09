use std::{fs, os::unix::fs::PermissionsExt};

use super::*;

/// A program that says its version and notes in `log` that it was started.
fn program(dir: &Path, version: &str) -> PathBuf {
    let path = dir.join("fake-tool");
    let script = format!(
        "#!/bin/sh\necho started >> '{}'\necho {version}\n",
        dir.join("log").display()
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn starts(dir: &Path) -> usize {
    fs::read_to_string(dir.join("log")).map_or(0, |log| log.lines().count())
}

#[test]
fn a_program_is_asked_its_version_once_for_the_file_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("graph.db");
    let open = || Graph::open(&db, lighthouse_graph::Options::default()).unwrap();
    let tool = program(dir.path(), "1.0").display().to_string();
    let ask = |graph: &mut Graph| {
        identities(graph, dir.path(), &[], std::slice::from_ref(&tool)).unwrap()
    };

    let mut graph = open();
    let first = ask(&mut graph);
    assert_eq!(ask(&mut graph), first);
    assert_eq!(starts(dir.path()), 1, "a second ask in the process");
    graph.commit().unwrap();
    drop(graph);

    let mut graph = open();
    assert_eq!(ask(&mut graph), first);
    assert_eq!(starts(dir.path()), 1, "an ask in another process");

    program(dir.path(), "2.0 and more");
    let changed = ask(&mut graph);
    assert_ne!(changed, first, "another file is another identity");
    assert_eq!(starts(dir.path()), 2);
}

#[test]
fn a_program_that_did_not_start_is_asked_again_in_the_next_run() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("graph.db");
    let mut graph = Graph::open(&db, lighthouse_graph::Options::default()).unwrap();
    let path = program(dir.path(), "1.0");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let tool = path.display().to_string();
    let ask = |graph: &mut Graph| {
        identities(graph, dir.path(), &[], std::slice::from_ref(&tool)).unwrap()
    };

    let refused = ask(&mut graph);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert_ne!(ask(&mut graph), refused);
    assert_eq!(starts(dir.path()), 1);
}
