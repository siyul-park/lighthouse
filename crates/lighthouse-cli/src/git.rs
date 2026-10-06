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

/// Whether tracked files differ from the commit the working tree is at.
/// Untracked files do not count; `false` when there is no repository.
pub fn dirty(root: &Path) -> bool {
    git(root, &["status", "--porcelain", "--untracked-files=no"])
        .is_ok_and(|out| !out.trim().is_empty())
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
