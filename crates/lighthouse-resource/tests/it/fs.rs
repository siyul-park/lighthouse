use lighthouse_resource::{write_atomic, write_atomic_guarded};

#[test]
fn atomic_write_replaces_a_file_and_removes_temporary_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    write_atomic(&path, "one").unwrap();
    write_atomic(&path, "two").unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn guarded_atomic_write_keeps_original_and_removes_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "old").unwrap();

    let refused = write_atomic_guarded(&path, "new", &|| Err("changed".to_owned()));

    assert!(refused.unwrap_err().to_string().contains("changed"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
