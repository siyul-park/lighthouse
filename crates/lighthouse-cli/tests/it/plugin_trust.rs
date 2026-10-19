//! A plugin the repository brings is a program, so it starts only in a project
//! the user has trusted, and a change to it withdraws that trust.

use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use predicates::prelude::*;
use tempfile::TempDir;

use crate::checks::{lighthouse, trust, write};

const MANIFEST: &str = "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Plugin\"\n[metadata]\nname = \"evil\"\n[spec]\nversion = \"1\"\n[spec.runtime]\ncommand = \"./run.sh\"\n";

/// A project that lists a plugin of its own, whose program marks the project
/// directory when it starts.
fn with_plugin() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project("plugins = [\"core\", \"evil\"]\n"),
    );
    write(dir.path(), "a.txt", "hello\n");
    write(
        dir.path(),
        ".lighthouse/plugins/evil/lighthouse-plugin.toml",
        MANIFEST,
    );
    set_program(&dir, "#!/bin/sh\ntouch ran\nexit 1\n");
    dir
}

fn set_program(dir: &TempDir, text: &str) {
    let run = dir.path().join(".lighthouse/plugins/evil/run.sh");
    fs::write(&run, text).unwrap();
    fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_plugin_of_the_project_does_not_start_until_the_user_trusts_the_project() {
    let dir = with_plugin();

    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("lighthouse trust"));
    assert!(!dir.path().join("ran").exists(), "the plugin started");

    trust(dir.path());
    lighthouse(dir.path()).arg("check").output().unwrap();
    assert!(dir.path().join("ran").exists(), "the plugin did not start");

    fs::remove_file(dir.path().join("ran")).unwrap();
    set_program(&dir, "#!/bin/sh\ntouch ran\nexit 2\n");
    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("lighthouse trust"));
    assert!(!dir.path().join("ran").exists(), "a changed plugin started");
}

/// The plugin `evil` installed in `at`, marking the project directory it is
/// started in.
fn install(at: &Path) {
    write(at, "lighthouse-plugin.toml", MANIFEST);
    let run = at.join("run.sh");
    fs::write(&run, "#!/bin/sh\ntouch ran\nexit 1\n").unwrap();
    fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_plugin_the_project_names_by_a_path_outside_it_needs_trust_too() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    install(&elsewhere.path().join("evil"));
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [\"core\", {{ id = \"evil\", path = {:?} }}]\n",
            elsewhere.path().join("evil")
        )),
    );
    write(dir.path(), "a.txt", "hello\n");

    lighthouse(dir.path())
        .arg("check")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("lighthouse trust"));
    assert!(!dir.path().join("ran").exists(), "the plugin started");

    trust(dir.path());
    lighthouse(dir.path()).arg("check").output().unwrap();
    assert!(dir.path().join("ran").exists(), "the plugin did not start");
}

#[test]
fn a_plugin_of_the_users_plugin_directory_starts_without_trust() {
    let dir = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    install(&data.path().join("lighthouse/plugins/evil"));
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project("plugins = [\"core\", \"evil\"]\n"),
    );
    write(dir.path(), "a.txt", "hello\n");

    lighthouse(dir.path())
        .env("XDG_DATA_HOME", data.path())
        .arg("check")
        .output()
        .unwrap();

    assert!(dir.path().join("ran").exists(), "the plugin did not start");
}
