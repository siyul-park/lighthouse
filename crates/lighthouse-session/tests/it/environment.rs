//! The layers of the configuration: the user's file and the repository-local
//! one over the project's.

use lighthouse_session::environment;

use crate::fixture::{canonical, project, repo, session, write};

#[test]
fn user_file() {
    crate::fixture::isolate();
    let file = environment::user_file().unwrap();
    assert!(file.ends_with("lighthouse/config.toml"), "{file:?}");
    assert!(file.starts_with(lighthouse_resource::var("XDG_CONFIG_HOME").unwrap()));
}

#[test]
fn local_file() {
    let dir = project("error");
    assert_eq!(
        environment::local_file(dir.path()),
        dir.path().join(".lighthouse/local.toml")
    );
}

#[test]
fn settings() {
    let dir = project("error");
    write(
        dir.path(),
        ".lighthouse/local.toml",
        "[fix]\nunsafe = true\n",
    );
    let session = session(dir.path());

    let plain = environment::settings(&session, false);
    let rule = plain
        .iter()
        .find(|s| s.key == "fix.unsafe")
        .unwrap_or_else(|| panic!("{plain:?}"));
    // The setting is the local file's.
    assert_eq!(rule.value, true);
    assert_eq!(rule.origin, None);

    let origin = environment::settings(&session, true);
    let rule = origin.iter().find(|s| s.key == "fix.unsafe").unwrap();
    assert_eq!(rule.origin.as_deref(), Some(".lighthouse/local.toml"));
    // What no file sets is the bundled default.
    assert!(
        origin
            .iter()
            .any(|s| s.origin.as_deref() == Some("bundled"))
    );
}

#[test]
fn a_committed_local_file_is_refused() {
    let dir = repo("error");
    write(
        dir.path(),
        ".lighthouse/local.toml",
        "[fix]\nunsafe = true\n",
    );
    super::fixture::git(dir.path(), &["add", "-f", ".lighthouse/local.toml"]);
    let refused = lighthouse_session::Session::find_in(&canonical(dir.path()))
        .err()
        .unwrap();
    assert!(
        refused
            .to_string()
            .contains("personal settings are committed")
    );
}
