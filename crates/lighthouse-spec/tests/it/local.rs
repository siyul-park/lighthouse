//! Where a project keeps its own decisions and how they are read.

#[test]
fn load_local() {
    let root = tempfile::tempdir().unwrap();
    assert!(lighthouse_spec::load_local(root.path()).unwrap().is_none());

    let dir = root.path().join(".lighthouse/decisions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("notes.txt"), "not a decision").unwrap();
    std::fs::write(dir.join("bad.yaml"), "id: other/x\n").unwrap();
    assert!(lighthouse_spec::load_local(root.path()).is_err());

    std::fs::remove_file(dir.join("bad.yaml")).unwrap();
    assert!(lighthouse_spec::load_local(root.path()).unwrap().is_some());
}

#[test]
fn local_files_and_local_dir() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        lighthouse_spec::local_dir(root.path()),
        root.path().join(".lighthouse/decisions")
    );
    assert!(lighthouse_spec::local_files(root.path()).unwrap().is_none());

    let dir = lighthouse_spec::local_dir(root.path());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.yaml"), "id: local/a\n").unwrap();
    std::fs::write(dir.join("notes.txt"), "not a decision").unwrap();
    let files = lighthouse_spec::local_files(root.path()).unwrap().unwrap();
    assert_eq!(files.keys().collect::<Vec<_>>(), ["a.yaml"]);
    assert_eq!(files["a.yaml"], "id: local/a\n");
}

#[test]
fn a_project_with_legacy_rules_is_refused_instead_of_ignored() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".lighthouse/rules")).unwrap();
    std::fs::create_dir_all(root.path().join(".lighthouse/decisions")).unwrap();

    for error in [
        lighthouse_spec::local_files(root.path())
            .unwrap_err()
            .to_string(),
        lighthouse_spec::load_local(root.path())
            .unwrap_err()
            .to_string(),
    ] {
        assert!(error.contains(".lighthouse/rules"), "{error}");
        assert!(error.contains("lighthouse spec migrate"), "{error}");
    }
}
