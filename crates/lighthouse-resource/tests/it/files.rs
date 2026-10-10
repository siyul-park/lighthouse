use lighthouse_resource::file_in;

#[test]
fn file_in_is_the_first_name_that_is_a_file() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(file_in(dir.path(), &["a.toml", "b.toml"]), None);

    std::fs::create_dir(dir.path().join("a.toml")).unwrap();
    std::fs::write(dir.path().join("b.toml"), "").unwrap();
    std::fs::write(dir.path().join("c.toml"), "").unwrap();

    assert_eq!(
        file_in(dir.path(), &["a.toml", "c.toml", "b.toml"]),
        Some(dir.path().join("c.toml")),
        "a directory is not a file, and the order of the names decides"
    );
}
