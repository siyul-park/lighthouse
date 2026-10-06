//! Report scope from git: which files a run reports on. Analysis scope never
//! changes; these only narrow the report.

use std::path::{Path, PathBuf};

use crate::{Result, git::git};

/// Files changed in the working tree against HEAD: modified, added, deleted,
/// renamed (under both names) and untracked. A deleted file stays in the
/// report scope so that the findings it had can be resolved. Project-relative,
/// sorted.
pub fn changed(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = tracked(
        root,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "--diff-filter=ACMRTD",
            "-z",
            "HEAD",
        ],
    )?;
    files.extend(untracked(root)?);
    finish(root, files)
}

/// Files changed since the merge base of `base` and HEAD, in the working tree,
/// deleted and renamed ones included, plus untracked files.
pub fn since(root: &Path, base: &str) -> Result<Vec<PathBuf>> {
    let merge_base = git(root, &["merge-base", base, "HEAD"])?;
    let merge_base = merge_base.trim();
    let mut files = tracked(
        root,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "--diff-filter=ACMRTD",
            "-z",
            merge_base,
        ],
    )?;
    files.extend(untracked(root)?);
    finish(root, files)
}

fn untracked(root: &Path) -> Result<Vec<String>> {
    tracked(
        root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "--full-name",
            "-z",
        ],
    )
}

fn tracked(root: &Path, args: &[&str]) -> Result<Vec<String>> {
    Ok(git(root, args)?
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Every path above is relative to the repository root; rebase them onto the
/// project root and drop what lies outside it.
fn finish(root: &Path, files: Vec<String>) -> Result<Vec<PathBuf>> {
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    let top = PathBuf::from(top.trim()).canonicalize()?;
    let root = root.canonicalize()?;
    let mut out: Vec<PathBuf> = files
        .into_iter()
        .filter_map(|file| top.join(file).strip_prefix(&root).ok().map(Path::to_owned))
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}
