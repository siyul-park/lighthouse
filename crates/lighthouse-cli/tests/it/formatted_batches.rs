//! `core/formatted` starts one formatter for the files of a run, and none when
//! nothing changed: a stand-in `gofmt` logs each run it is given.

use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use assert_cmd::Command;

use crate::{
    cache_fixtures::write,
    checks::{home, trust},
};

/// Lists the files that hold the word UNFORMATTED, as `gofmt -l` lists those
/// it would change, and logs the arguments of each run.
const GOFMT: &str = "#!/bin/sh\necho \"$*\" >> \"LOG\"\nfor f in \"$@\"; do\n  case $f in -*) continue ;; esac\n  grep -q UNFORMATTED \"$f\" && echo \"$f\"\ndone\nexit 0\n";

/// A project, and, beside it, the stand-in and its log: the project's files
/// are what a run is keyed on, and the log is not one of them.
struct Project {
    dir: tempfile::TempDir,
    tools: tempfile::TempDir,
    log: PathBuf,
}

fn project() -> Option<Project> {
    let plugin = lighthouse_test_support::lang_go()?;
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-go\", path = {:?} }}]\n[sandbox]\nenabled = false\n[rules]\n\"core/formatted\" = \"error\"\n",
            plugin.to_str().unwrap()
        )),
    );
    write(dir.path(), "go.mod", "module demo\n\ngo 1.22\n");
    write(
        dir.path(),
        "a.go",
        "package demo\n\n// A is a.\nfunc A() {}\n",
    );
    write(
        dir.path(),
        "b.go",
        "package demo\n\n// B is b.\nfunc B() {}\n",
    );
    write(
        dir.path(),
        "c.go",
        "package demo\n\n// C is c. UNFORMATTED\nfunc C() {}\n",
    );
    let tools = tempfile::tempdir().unwrap();
    let log = tools.path().join("gofmt.log");
    write(
        tools.path(),
        "gofmt",
        &GOFMT.replace("LOG", log.to_str().unwrap()),
    );
    fs::set_permissions(
        tools.path().join("gofmt"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    trust(dir.path());
    Some(Project { dir, tools, log })
}

fn check(project: &Project) -> String {
    let dir = project.dir.path();
    let path = env::join_paths(
        [project.tools.path().to_owned()]
            .into_iter()
            .chain(env::split_paths(&env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let out = Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(dir)
        .envs(home(dir))
        .env_remove("LIGHTHOUSE_TRUST")
        .env("PATH", path)
        .args(["check", "--no-store", "--format", "json", "--decision"])
        .arg("core/formatted")
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

/// The runs of the stand-in that were given files.
fn runs(log: &Path) -> Vec<String> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(".go"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn one_run_formats_every_file_and_a_run_without_changes_starts_none() {
    let Some(project) = project() else { return };

    let first = check(&project);
    let after_first = runs(&project.log);
    assert_eq!(after_first.len(), 1, "{after_first:?}");
    for file in ["a.go", "b.go", "c.go"] {
        assert!(after_first[0].contains(file), "{after_first:?}");
    }
    assert!(
        first.contains("c.go") && !first.contains("\"a.go\""),
        "{first}"
    );

    assert_eq!(check(&project), first);
    assert_eq!(
        runs(&project.log),
        after_first,
        "a warm run started a formatter"
    );

    write(
        project.dir.path(),
        "a.go",
        "package demo\n\n// A is a, changed.\nfunc A() {}\n",
    );
    assert_eq!(check(&project), first);
    assert_eq!(
        runs(&project.log).len(),
        2,
        "one run for the changed project"
    );
}
