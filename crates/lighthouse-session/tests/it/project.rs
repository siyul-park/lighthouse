//! Opening a project: the session, its registry and engine, and the files
//! around it.

use std::fs;

use lighthouse_session::Session;

use crate::fixture::{canonical, in_dir, isolate, project, read, session, trusted, write};

#[test]
fn session_load() {
    let dir = project("error");
    let nested = dir.path().join("deep/er");
    fs::create_dir_all(&nested).unwrap();

    let found = in_dir(&nested, || Session::load(None)).unwrap();
    assert_eq!(canonical(&found.root), canonical(dir.path()));
    assert!(found.catalog.decision("local/short").is_some());

    let empty = tempfile::tempdir().unwrap();
    let missing = in_dir(empty.path(), || Session::load(None)).err().unwrap();
    assert!(
        missing.to_string().contains("no lighthouse.toml found"),
        "{missing}"
    );

    write(dir.path(), "other.toml", "not a project");
    let broken = in_dir(dir.path(), || {
        Session::load(Some(&dir.path().join("other.toml")))
    });
    assert!(broken.is_err());
}

#[test]
fn session_load_or_default() {
    isolate();
    let empty = tempfile::tempdir().unwrap();
    let default = in_dir(empty.path(), || Session::load_or_default(None)).unwrap();
    assert_eq!(canonical(&default.root), canonical(empty.path()));
    assert!(default.config.plugins().is_empty());
    assert!(default.catalog.decision("local/short").is_none());

    let dir = project("error");
    let own = in_dir(dir.path(), || Session::load_or_default(None)).unwrap();
    assert!(own.catalog.decision("local/short").is_some());
}

#[test]
fn session_trusted() {
    let dir = project("error");
    assert!(!session(dir.path()).trusted());

    assert!(trusted(dir.path()).trusted());

    // Whatever changes in what would run asks again.
    let short = read(dir.path(), ".lighthouse/decisions/short.yaml");
    write(
        dir.path(),
        ".lighthouse/decisions/short.yaml",
        &short.replace("head -n 2", "head -n 1"),
    );
    assert!(!session(dir.path()).trusted());
}

#[test]
fn session_registry() {
    let dir = project("error");
    let (registry, registered) = session(dir.path()).registry().unwrap();
    assert!(registry.rule("local/short").is_some());
    assert!(registry.rule("core/max-lines").is_some());
    assert!(registered.incomplete.is_empty());
}

#[test]
fn session_engine() {
    let dir = project("error");
    let (engine, notices) = session(dir.path()).engine().unwrap();
    assert!(engine.active("local/short"));
    assert!(notices.is_empty(), "{notices:?}");

    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project("[sandbox]\nenabled = false\n"),
    );
    let refused = session(dir.path()).engine().err().unwrap();
    assert!(
        refused.to_string().contains("needs a trusted project"),
        "{refused}"
    );
    let (_, notices) = trusted(dir.path()).engine().unwrap();
    assert!(
        notices.iter().any(|n| n.contains("sandbox off")),
        "{notices:?}"
    );
}

#[test]
fn session_in_process_registry() {
    let dir = project("error");
    let registry = session(dir.path()).in_process_registry().unwrap();
    assert!(registry.rule("local/short").is_some());
    assert!(registry.rule("no/such-rule").is_none());
}

#[test]
fn project_root() {
    let dir = project("error");
    let nested = dir.path().join("a/b");
    fs::create_dir_all(&nested).unwrap();
    let up = in_dir(&nested, || lighthouse_session::project_root(None)).unwrap();
    assert_eq!(canonical(&up), canonical(dir.path()));
    let named = in_dir(&nested, || {
        lighthouse_session::project_root(Some(&dir.path().join("lighthouse.toml")))
    })
    .unwrap();
    assert_eq!(canonical(&named), canonical(&nested));
}

#[test]
fn config_file() {
    let dir = project("error");
    assert_eq!(
        lighthouse_session::config_file(dir.path()),
        Some(dir.path().join("lighthouse.toml"))
    );
    assert_eq!(
        lighthouse_session::config_file(&dir.path().join(".lighthouse")),
        None
    );
}

#[test]
fn catalog_at() {
    let dir = project("error");
    let catalog = lighthouse_session::catalog_at(dir.path()).unwrap();
    assert!(catalog.decision("local/short").is_some());
    assert!(catalog.decision("core/max-lines").is_some());

    write(
        dir.path(),
        ".lighthouse/decisions/broken.yaml",
        "kind: Decision\nmetadata: {}\n",
    );
    assert!(lighthouse_session::catalog_at(dir.path()).is_err());
}
