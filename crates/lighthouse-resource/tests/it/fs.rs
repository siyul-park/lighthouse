use lighthouse_resource::{read_optional, write_atomic, write_atomic_guarded};

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

#[test]
fn atomic_write_creates_the_directories_above_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join("b").join("c.txt");

    write_atomic(&path, "deep").unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "deep");
}

#[test]
fn a_missing_file_reads_as_none_and_a_present_one_as_its_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    assert_eq!(read_optional(&path).unwrap(), None);

    std::fs::write(&path, "here").unwrap();

    assert_eq!(read_optional(&path).unwrap().as_deref(), Some("here"));
    assert!(read_optional(dir.path()).is_err(), "a directory is no text");
}
