//! What `--version` prints, and where an installed executable finds its plugins.

use std::{fs, path::Path};

use assert_cmd::Command;
use predicates::prelude::*;

use crate::checks::home;

#[test]
fn a_default_build_prints_the_cargo_version_marked_dev() {
    Command::cargo_bin("lighthouse")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("lighthouse {}-dev\n", env!("CARGO_PKG_VERSION")));
}

/// A project that lists the plugin `ghost`, which only `plugins/ghost` next to
/// the executable provides; its program does not exist, so finding it is told
/// from not finding it by the message.
fn project_with_ghost(dir: &Path) {
    fs::write(
        dir.join("lighthouse.toml"),
        lighthouse_test_support::project("plugins = [\"ghost\"]\n"),
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn a_symlinked_executable_finds_the_plugins_next_to_its_real_path() {
    let install = tempfile::tempdir().unwrap();
    let real = install.path().join("lib");
    let plugin = real.join("plugins/ghost");
    fs::create_dir_all(&plugin).unwrap();
    fs::write(
        plugin.join("lighthouse-plugin.toml"),
        "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Plugin\"\n[metadata]\nname = \"ghost\"\n[spec]\nversion = \"1\"\n[spec.runtime]\ncommand = \"/does/not/exist\"\n",
    )
    .unwrap();
    fs::copy(
        assert_cmd::cargo::cargo_bin("lighthouse"),
        real.join("lighthouse"),
    )
    .unwrap();
    let bin = install.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(real.join("lighthouse"), bin.join("lighthouse")).unwrap();

    let project = tempfile::tempdir().unwrap();
    project_with_ghost(project.path());
    fs::write(project.path().join("a.txt"), "a\n").unwrap();
    Command::new(bin.join("lighthouse"))
        .current_dir(project.path())
        .envs(home(project.path()))
        .env("LIGHTHOUSE_TRUST", "1")
        .arg("check")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown plugin").not());
}
