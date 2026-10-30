//! Validating spec documents, and the examples of the decisions among them.

use crate::fixture::{project, read, trusted, write};

#[test]
fn validate_paths() {
    let dir = project("error");
    let session = trusted(dir.path());
    let decisions = dir.path().join(".lighthouse/decisions");

    let valid =
        lighthouse_session::validate_paths(&session, std::slice::from_ref(&decisions), true)
            .unwrap();
    assert_eq!((valid.files, valid.documents), (1, 1));
    assert!(valid.problems.is_empty(), "{:?}", valid.problems);

    // A check that disagrees with its examples is reported with the example.
    let text = read(dir.path(), ".lighthouse/decisions/short.yaml");
    write(
        dir.path(),
        ".lighthouse/decisions/short.yaml",
        &text.replace("file.lines > 2", "file.lines > 5"),
    );
    let session = trusted(dir.path());
    let by_example =
        lighthouse_session::validate_paths(&session, std::slice::from_ref(&decisions), true)
            .unwrap();
    assert!(!by_example.problems.is_empty());
    let without =
        lighthouse_session::validate_paths(&session, std::slice::from_ref(&decisions), false)
            .unwrap();
    assert!(without.problems.is_empty(), "{:?}", without.problems);

    // A document that lacks a field, and a path nothing holds.
    write(
        dir.path(),
        ".lighthouse/decisions/broken.yaml",
        "apiVersion: lighthouse/v1alpha1\nkind: Decision\nmetadata:\n  name: local/broken\nspec:\n  title: Broken\n",
    );
    let broken = lighthouse_session::validate_paths(&session, &[decisions], false).unwrap();
    assert!(
        broken
            .problems
            .iter()
            .any(|p| p.message.starts_with("broken.yaml: missing field")),
        "{:?}",
        broken.problems
    );
    let missing = lighthouse_session::validate_paths(&session, &[dir.path().join("none")], false);
    assert!(missing.err().unwrap().to_string().contains("no such file"));
}
