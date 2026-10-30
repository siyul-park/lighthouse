//! Which files a run reports on, from the repository.

use std::{fs, path::PathBuf};

use crate::fixture::{git, project, repo, write};

#[test]
fn head() {
    let dir = repo("error");
    let commit = lighthouse_session::head(dir.path()).unwrap();
    assert_eq!(commit, git(dir.path(), &["rev-parse", "HEAD"]).trim());
    assert_eq!(commit.len(), 40);

    assert_eq!(lighthouse_session::head(project("error").path()), None);
}

#[test]
fn changed() {
    let dir = repo("error");
    assert!(lighthouse_session::changed(dir.path()).unwrap().is_empty());

    write(dir.path(), "a.txt", "hello again\n");
    write(dir.path(), "new.txt", "new\n");
    fs::remove_file(dir.path().join(".gitignore")).unwrap();
    assert_eq!(
        lighthouse_session::changed(dir.path()).unwrap(),
        [".gitignore", "a.txt", "new.txt"].map(PathBuf::from)
    );

    assert!(lighthouse_session::changed(project("error").path()).is_err());
}

#[test]
fn since() {
    let dir = repo("error");
    git(dir.path(), &["checkout", "-q", "-b", "feature"]);
    write(dir.path(), "b.txt", "b\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "second"]);
    write(dir.path(), "c.txt", "c\n");

    assert_eq!(
        lighthouse_session::since(dir.path(), "main").unwrap(),
        ["b.txt", "c.txt"].map(PathBuf::from)
    );
    assert!(lighthouse_session::since(dir.path(), "--output=x").is_err());
    assert!(lighthouse_session::since(dir.path(), "no-such-ref").is_err());
}
