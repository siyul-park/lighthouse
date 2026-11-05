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

use super::{Raw, Reading};

/// The ignore files of one directory.
pub(super) struct Level {
    gitignore: Gitignore,
    exclude: Gitignore,
}

impl Level {
    /// The ignore files of the directory at `rel` (`abs` on disk) that holds
    /// `entries`: the text of `.gitignore` comes from `reading`, the
    /// `info/exclude` of a repository from disk. What cannot be parsed is
    /// pushed to `errors`.
    pub(super) fn of(
        rel: &Path,
        abs: &Path,
        entries: &[Raw],
        reading: &mut Reading,
        errors: &mut Vec<String>,
    ) -> Self {
        let has = |name: &str| entries.iter().any(|raw| raw.name == name);
        let mut compile = |file: &Path, text: Option<String>| {
            let mut builder = GitignoreBuilder::new(abs);
            if let Some(text) = text {
                for (i, line) in text.lines().enumerate() {
                    let line = if i == 0 {
                        line.trim_start_matches('\u{feff}')
                    } else {
                        line
                    };
                    if let Err(e) = builder.add_line(Some(file.to_owned()), line) {
                        errors.push(e.to_string());
                    }
                }
            }
            builder.build().unwrap_or_else(|e| {
                errors.push(e.to_string());
                Gitignore::empty()
            })
        };
        let gitignore = if has(".gitignore") {
            let at = rel.join(".gitignore");
            compile(&abs.join(".gitignore"), reading.text(&at))
        } else {
            Gitignore::empty()
        };
        let exclude = match has(".git").then(|| git_dir(&abs.join(".git"))).flatten() {
            Some(dir) => {
                let file = dir.join("info/exclude");
                let text = fs::read_to_string(&file).ok();
                compile(&file, text)
            }
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
