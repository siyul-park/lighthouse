//! `markdown/gfm` and `markdown/links` keep the finding of a file: a run in
//! which nothing changed starts no program, an edit of one file starts
//! markdownlint for that file alone, and lychee, which reads what a link names,
//! runs again for the files when the project's files or its Markdown change.
//! Stand-ins log each run they are given.

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

/// Fails (exit 1) on a file that holds the word BAD, as the real tools do on a
/// finding, and logs the arguments of each run.
const STAND_IN: &str = "#!/bin/sh\necho \"$*\" >> \"LOG\"\nfor f in \"$@\"; do\n  case $f in -*) continue ;; esac\n  grep -q BAD \"$f\" && exit 1\ndone\nexit 0\n";

struct Project {
    dir: tempfile::TempDir,
    tools: tempfile::TempDir,
}

fn project() -> Project {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "lighthouse.toml",
        &lighthouse_test_support::project(&format!(
            "plugins = [{{ id = \"lang-markdown\", path = {:?} }}]\n[sandbox]\nenabled = false\n[rules]\n\"markdown/gfm\" = \"warn\"\n\"markdown/links\" = \"warn\"\n",
            lighthouse_test_support::lang_markdown().to_str().unwrap()
        )),
    );
    write(dir.path(), "a.md", "# A\n");
    write(dir.path(), "b.md", "# B BAD\n");
    write(dir.path(), "c.md", "# C\n");
    let tools = tempfile::tempdir().unwrap();
    for tool in ["markdownlint-cli2", "lychee"] {
        let log = tools.path().join(format!("{tool}.log"));
        write(
            tools.path(),
            tool,
            &STAND_IN.replace("LOG", log.to_str().unwrap()),
        );
        fs::set_permissions(tools.path().join(tool), fs::Permissions::from_mode(0o755)).unwrap();
    }
    trust(dir.path());
    Project { dir, tools }
}

fn check(project: &Project) -> String {
    let path = env::join_paths(
        [project.tools.path().to_owned()]
            .into_iter()
            .chain(env::split_paths(&env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let out = Command::cargo_bin("lighthouse")
        .unwrap()
        .current_dir(project.dir.path())
        .envs(home(project.dir.path()))
        .env_remove("LIGHTHOUSE_TRUST")
        .env("PATH", path)
        .args(["check", "--no-store", "--format", "json", "--decision"])
        .arg("markdown/gfm")
        .arg("--decision")
        .arg("markdown/links")
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

/// The runs of `tool` so far, one line each.
fn runs(project: &Project, tool: &str) -> Vec<String> {
    let log: PathBuf = project.tools.path().join(format!("{tool}.log"));
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(".md"))
        .map(str::to_owned)
        .collect()
}

fn touch(dir: &Path, name: &str, text: &str) {
    write(dir, name, text);
}

#[test]
fn a_run_without_changes_starts_no_program_and_answers_the_same() {
    let project = project();

    let first = check(&project);
    assert_eq!(runs(&project, "markdownlint-cli2").len(), 3);
    assert_eq!(runs(&project, "lychee").len(), 3);
    assert!(
        first.contains("b.md") && !first.contains("\"a.md\""),
        "{first}"
    );

    assert_eq!(check(&project), first);
    assert_eq!(
        runs(&project, "markdownlint-cli2").len(),
        3,
        "gfm ran again"
    );
    assert_eq!(runs(&project, "lychee").len(), 3, "links ran again");
}

#[test]
fn an_edit_runs_markdownlint_for_that_file_alone() {
    let project = project();
    let first = check(&project);

    touch(project.dir.path(), "a.md", "# A, changed\n");

    assert_eq!(check(&project), first);
    let gfm = runs(&project, "markdownlint-cli2");
    assert_eq!(gfm.len(), 4, "{gfm:?}");
    assert!(gfm[3].contains("a.md"), "{gfm:?}");
}

#[test]
fn links_run_again_when_a_file_they_might_name_appears() {
    let project = project();
    let first = check(&project);

    touch(project.dir.path(), "d.txt", "a file a link may name\n");

    assert_eq!(check(&project), first);
    assert_eq!(runs(&project, "lychee").len(), 6, "links of each file");
    assert_eq!(
        runs(&project, "markdownlint-cli2").len(),
        3,
        "a file that is not Markdown does not change gfm"
    );
}
