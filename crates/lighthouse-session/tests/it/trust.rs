//! Trust is the user's: recorded for exactly what the project would run.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::fixture::{
    canonical, git, grant, isolate, project, project_with, read, session, withdraw, write,
};

const VERIFY: &str = "[fix]\nverify = [[\"sh\", \"check.sh\"]]\n";

#[test]
fn basis_trusted() {
    let dir = project("error");
    let before = session(dir.path());
    assert!(!before.basis.trusted(&before.root));
    assert!(
        before
            .basis
            .commands
            .iter()
            .any(|c| c.contains("fixer of local/short")),
        "{:?}",
        before.basis.commands
    );
    grant(&before);
    assert!(before.basis.trusted(&before.root));
}

#[test]
fn trust() {
    let dir = project("error");
    let before = session(dir.path());
    assert!(!before.trusted());

    let file = grant(&before);
    assert!(file.ends_with("trust.toml"), "{file:?}");
    assert!(session(dir.path()).trusted());

    // The decision's fix is a command: changing it asks again.
    let short = read(dir.path(), ".lighthouse/decisions/short.yaml");
    write(
        dir.path(),
        ".lighthouse/decisions/short.yaml",
        &short.replace("head -n 2", "head -n 1"),
    );
    assert!(!session(dir.path()).trusted());
}

#[test]
fn revoke() {
    let dir = project("error");
    let before = session(dir.path());
    assert!(!withdraw(&before.root));

    grant(&before);
    assert!(session(dir.path()).trusted());
    assert!(withdraw(&before.root));
    assert!(!session(dir.path()).trusted());
}

/// A linked worktree of the repository at `main`, on a branch of its own; the
/// worktree is the `wt` directory of the result.
fn worktree(main: &Path, branch: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let at = dir.path().join("wt");
    git(
        main,
        &["worktree", "add", "-q", "-b", branch, at.to_str().unwrap()],
    );
    dir
}

fn root(dir: &TempDir) -> PathBuf {
    dir.path().join("wt")
}

/// A committed repository whose configuration runs a program inside the
/// project, so that the digest holds a path under the root.
fn with_program() -> TempDir {
    isolate();
    let dir = project_with("error", VERIFY);
    write(dir.path(), "check.sh", "exit 0\n");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Kim"]);
    git(dir.path(), &["config", "user.email", "kim@example.com"]);
    write(
        dir.path(),
        ".gitignore",
        ".lighthouse/*.db*\n.lighthouse/cache\n",
    );
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "first"]);
    dir
}

#[test]
fn a_worktree_with_identical_inputs_is_trusted() {
    let main = with_program();
    assert!(!session(main.path()).trusted());
    grant(&session(main.path()));
    let linked = worktree(main.path(), "linked");
    assert!(session(&root(&linked)).trusted());
}

#[test]
fn a_worktree_with_one_changed_byte_is_not_trusted() {
    let main = with_program();
    grant(&session(main.path()));

    let config = worktree(main.path(), "config");
    let text = read(&root(&config), "lighthouse.toml");
    write(&root(&config), "lighthouse.toml", &format!("{text}# x\n"));
    assert!(!session(&root(&config)).trusted());

    let rule = worktree(main.path(), "rule");
    let text = read(&root(&rule), ".lighthouse/decisions/short.yaml");
    write(
        &root(&rule),
        ".lighthouse/decisions/short.yaml",
        &text.replace("head -n 2", "head -n 1"),
    );
    assert!(!session(&root(&rule)).trusted());

    let layer = worktree(main.path(), "layer");
    write(
        &root(&layer),
        ".lighthouse/local.toml",
        "[fix]\nunsafe = true\n",
    );
    assert!(!session(&root(&layer)).trusted());

    let program = worktree(main.path(), "program");
    write(&root(&program), "check.sh", "exit 1\n");
    assert!(!session(&root(&program)).trusted());
}

#[test]
fn an_unrelated_repository_with_identical_content_is_not_trusted() {
    let main = with_program();
    grant(&session(main.path()));
    let other = with_program();
    assert!(!session(other.path()).trusted());
}

#[test]
fn pointing_git_at_a_trusted_repository_grants_nothing() {
    let main = with_program();
    grant(&session(main.path()));
    let git_dir = canonical(main.path()).join(".git");

    // A `.git` file that names the trusted repository directly.
    let hostile = project_with("error", VERIFY);
    write(hostile.path(), "check.sh", "touch pwned\n");
    write(
        hostile.path(),
        ".git",
        &format!("gitdir: {}\n", git_dir.display()),
    );
    assert!(!session(hostile.path()).trusted());

    // One that names a directory whose `commondir` names it.
    let via = tempfile::tempdir().unwrap();
    write(
        via.path(),
        "gitdir/commondir",
        &format!("{}\n", git_dir.display()),
    );
    write(
        hostile.path(),
        ".git",
        &format!("gitdir: {}\n", via.path().join("gitdir").display()),
    );
    assert!(!session(hostile.path()).trusted());

    // Byte-identical inputs are trusted wherever they are: that is the rule,
    // and it grants what the digest already granted.
    let same = project_with("error", VERIFY);
    write(same.path(), "check.sh", "exit 0\n");
    write(
        same.path(),
        ".git",
        &format!("gitdir: {}\n", git_dir.display()),
    );
    assert!(session(same.path()).trusted());
}

#[test]
fn a_directory_outside_git_is_trusted_by_its_own_root() {
    let dir = project("error");
    let other = project("error");
    grant(&session(dir.path()));
    assert!(session(dir.path()).trusted());
    assert!(!session(other.path()).trusted());
}

#[test]
fn revoking_in_a_worktree_leaves_the_main_tree_trusted() {
    let main = with_program();
    grant(&session(main.path()));
    let linked = worktree(main.path(), "linked");
    let there = session(&root(&linked));
    grant(&there);
    assert!(withdraw(&there.root));
    assert!(!withdraw(&there.root));
    assert!(session(main.path()).trusted());
    // Still trusted through the main tree's entry, until that goes too.
    assert!(session(&root(&linked)).trusted());
    assert!(withdraw(&session(main.path()).root));
    assert!(!session(&root(&linked)).trusted());
}
