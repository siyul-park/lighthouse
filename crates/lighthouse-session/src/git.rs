//! Running git in the project.

use std::{path::Path, process::Command};

use crate::Result;

/// The commit the working tree is at, when the project is in a git repository
/// that has one.
pub fn head(root: &Path) -> Option<String> {
    git(root, &["rev-parse", "HEAD"])
        .ok()
        .map(|out| out.trim().to_owned())
}

/// The `user.name` configured for the repository, when set.
pub fn user_name(root: &Path) -> Option<String> {
    git(root, &["config", "user.name"])
        .ok()
        .map(|out| out.trim().to_owned())
        .filter(|name| !name.is_empty())
}

/// The commit the working tree is at, when the project is in a git repository
/// that has one, and whether tracked files differ from it (untracked files do
/// not count; `false` when there is no repository), from one run of git.
pub fn state(root: &Path) -> (Option<String>, bool) {
    let Ok(out) = git(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=no",
        ],
    ) else {
        return (None, false);
    };
    let (mut commit, mut dirty) = (None, false);
    for line in out.lines() {
        match line.strip_prefix("# branch.oid ") {
            Some(oid) => commit = Some(oid.trim()).filter(|oid| *oid != "(initial)"),
            None => dirty |= !line.starts_with('#'),
        }
    }
    (commit.map(str::to_owned), dirty)
}

/// Runs `git -C root args` and returns its stdout.
pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), why.trim()).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
