//! The ignore files that apply to a directory of the walk, and what they say
//! of its entries: the `ignore` crate's rules for `.gitignore` files from the
//! root down and for the `info/exclude` of each repository found.

use std::{
    fs,
    path::{Path, PathBuf},
};

use ignore::{
    Match,
    gitignore::{Gitignore, GitignoreBuilder},
};

use super::Raw;

/// The ignore files of one directory.
pub(super) struct Level {
    gitignore: Gitignore,
    exclude: Gitignore,
}

impl Level {
    /// The ignore files of the directory at `abs` that holds `entries`; what
    /// cannot be read or parsed is pushed to `errors`.
    pub(super) fn of(abs: &Path, entries: &[Raw], errors: &mut Vec<String>) -> Self {
        let has = |name: &str| entries.iter().any(|raw| raw.name == name);
        let mut compile = |file: &Path| {
            let mut builder = GitignoreBuilder::new(abs);
            if file.exists() {
                errors.extend(builder.add(file).map(|e| e.to_string()));
            }
            builder.build().unwrap_or_else(|e| {
                errors.push(e.to_string());
                Gitignore::empty()
            })
        };
        let gitignore = if has(".gitignore") {
            compile(&abs.join(".gitignore"))
        } else {
            Gitignore::empty()
        };
        let exclude = match has(".git").then(|| git_dir(&abs.join(".git"))).flatten() {
            Some(dir) => compile(&dir.join("info/exclude")),
            None => Gitignore::empty(),
        };
        Self { gitignore, exclude }
    }
}

/// Whether the deepest `.gitignore`, then the deepest `info/exclude`, that
/// has a say ignores the path.
pub(super) fn ignored(chain: &[Level], path: &Path, is_dir: bool) -> bool {
    let mut found = Match::None;
    for level in chain.iter().rev() {
        found = level.gitignore.matched(path, is_dir);
        if !found.is_none() {
            return found.is_ignore();
        }
    }
    for level in chain.iter().rev() {
        found = level.exclude.matched(path, is_dir);
        if !found.is_none() {
            break;
        }
    }
    found.is_ignore()
}

/// Where `info/exclude` is for the `.git` at `git`: the directory itself, or
/// for the file of a worktree the common directory of the one it names.
fn git_dir(git: &Path) -> Option<PathBuf> {
    if !fs::metadata(git).ok()?.is_file() {
        return Some(git.to_owned());
    }
    let text = fs::read_to_string(git).ok()?;
    let real = PathBuf::from(text.lines().next()?.strip_prefix("gitdir: ")?);
    let common = fs::read_to_string(real.join("commondir")).ok()?;
    let common = common.lines().next()?;
    Some(if common.starts_with('.') {
        real.join(common)
    } else {
        PathBuf::from(common)
    })
}
