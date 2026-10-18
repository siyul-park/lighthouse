//! The copies a run is given: of a file, and of a project.

use std::{collections::BTreeMap, fs, path::PathBuf};

use lighthouse_process::{clone_file, copy_project};

#[test]
fn clone_file_copies_the_bytes_and_leaves_the_source_alone() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("from.txt");
    let to = dir.path().join("to.txt");
    fs::write(&from, "one").unwrap();

    clone_file(&from, &to).unwrap();
    fs::write(&to, "two").unwrap();

    assert_eq!(fs::read_to_string(&from).unwrap(), "one");
    assert_eq!(fs::read_to_string(&to).unwrap(), "two");
}

#[test]
fn copy_project_copies_the_files_but_not_the_secrets_or_the_repository() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join("src/a.txt"), "a").unwrap();
    fs::write(root.join(".env"), "KEY=secret").unwrap();
    fs::write(root.join(".git/HEAD"), "ref").unwrap();
    let into = tempfile::tempdir().unwrap();

    let placed = copy_project(root, into.path(), &BTreeMap::new()).unwrap();

    assert_eq!(placed, [PathBuf::from("src/a.txt")].into());
    assert_eq!(
        fs::read_to_string(into.path().join("src/a.txt")).unwrap(),
        "a"
    );
    assert!(!into.path().join(".env").exists());
    assert!(!into.path().join(".git").exists());
}

#[test]
fn copy_project_writes_the_texts_over_the_files_and_refuses_a_path_that_leaves_the_project() {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("a.txt"), "disk").unwrap();
    let into = tempfile::tempdir().unwrap();
    let texts = BTreeMap::from([
        (PathBuf::from("a.txt"), "overlay".to_owned()),
        (PathBuf::from("new/b.txt"), "new".to_owned()),
    ]);

    copy_project(project.path(), into.path(), &texts).unwrap();

    assert_eq!(
        fs::read_to_string(project.path().join("a.txt")).unwrap(),
        "disk"
    );
    assert_eq!(
        fs::read_to_string(into.path().join("a.txt")).unwrap(),
        "overlay"
    );
    assert_eq!(
        fs::read_to_string(into.path().join("new/b.txt")).unwrap(),
        "new"
    );
    let outside = BTreeMap::from([(PathBuf::from("../escape.txt"), "x".to_owned())]);
    assert!(copy_project(project.path(), into.path(), &outside).is_err());
}
