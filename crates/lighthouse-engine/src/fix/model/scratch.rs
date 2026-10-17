//! The scratch copy of the project, which the commands of `[fix] verify` and
//! an agent that edits run in, so that the files themselves stay as they were
//! until the fixes are accepted. The copy holds the fixed texts of the run in
//! place of the files they name, and is deleted when it is dropped.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use ignore::WalkBuilder;
use lighthouse_process::{GRACE, Invocation, MAX_OUTPUT};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::Overlays;

/// Lines of the output of a failed command that the notes keep.
const TAIL_LINES: usize = 20;

/// Changes to files that are not edits of a text that an agent made, after
/// which looking for more is pointless: any of them rejects its work.
const MAX_OPAQUE: usize = 32;

/// A copy of the project in a directory of its own.
pub(in crate::fix) struct Scratch {
    dir: TempDir,
    /// The files copied, project-relative.
    copied: BTreeSet<PathBuf>,
}

/// What became of a file of the scratch copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::fix) enum Change {
    /// The file holds this text now.
    Text(String),
    /// The file changed and is not text.
    Binary,
    /// The file was not in the copy.
    Created,
    /// The file is no longer there, or no longer a file.
    Deleted,
}

/// The files of a scratch copy that differ from the project, by path.
pub(in crate::fix) type Changes = BTreeMap<PathBuf, Change>;

impl Scratch {
    /// A copy of the files of the project at `root` that version control
    /// would keep, with `texts` written over the files they name.
    pub(in crate::fix) fn new(root: &Path, texts: &Overlays) -> io::Result<Self> {
        let dir = tempfile::tempdir()?;
        let copied = copy_project(root, dir.path(), texts)?;
        Ok(Self { dir, copied })
    }

    pub(in crate::fix) fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The files whose content differs from the project at `root` as `texts`
    /// stand over it, those created and those removed. Stops looking after a
    /// few changes that are not edits of a text: they say enough.
    pub(in crate::fix) fn changes(&self, root: &Path, texts: &Overlays) -> io::Result<Changes> {
        let mut found = Changes::new();
        let mut present = BTreeSet::new();
        let walk = WalkBuilder::new(self.path())
            .standard_filters(false)
            .sort_by_file_path(Ord::cmp)
            .build();
        for entry in walk {
            let entry = entry.map_err(io::Error::other)?;
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let Ok(relative) = entry.path().strip_prefix(self.path()) else {
                continue;
            };
            present.insert(relative.to_owned());
            let change = if self.copied.contains(relative) {
                let now = fs::read(entry.path())?;
                if now == baseline(root, texts, relative)? {
                    continue;
                }
                String::from_utf8(now).map_or(Change::Binary, Change::Text)
            } else {
                Change::Created
            };
            found.insert(relative.to_owned(), change);
            if opaque(&found) >= MAX_OPAQUE {
                return Ok(found);
            }
        }
        for gone in self.copied.difference(&present) {
            found.insert(gone.clone(), Change::Deleted);
        }
        Ok(found)
    }
}

/// Runs `commands` in turn over a copy of the project at `root` with `texts`
/// in place of the files they name. The first that fails ends the run: its
/// text and the tail of its output are the error.
pub(in crate::fix) fn run_verify(
    root: &Path,
    texts: &Overlays,
    commands: &[Vec<String>],
    limit: Duration,
) -> Result<(), String> {
    let scratch = Scratch::new(root, texts)
        .map_err(|e| format!("the scratch copy of the project failed: {e}"))?;
    for argv in commands {
        let shown = argv.join(" ");
        let ran = lighthouse_process::run(&Invocation {
            argv,
            dir: scratch.path(),
            stdin: &[],
            env: None,
            limit,
            grace: GRACE,
            max_output: MAX_OUTPUT,
        })
        .map_err(|reason| format!("`{shown}`: {reason}"))?;
        if !ran.success {
            let how = ran.code.map_or("was ended by a signal".to_owned(), |c| {
                format!("exited {c}")
            });
            let (stderr, stdout) = (tail(&ran.stderr), tail(&ran.stdout));
            let output = if stderr.is_empty() { stdout } else { stderr };
            return Err(format!("`{shown}` {how}:\n{output}"));
        }
    }
    Ok(())
}

fn opaque(changes: &Changes) -> usize {
    changes
        .values()
        .filter(|change| !matches!(change, Change::Text(_)))
        .count()
}

/// What the file `relative` held when the copy was made: the text standing in
/// for it, else the file itself.
fn baseline(root: &Path, texts: &Overlays, relative: &Path) -> io::Result<Vec<u8>> {
    match texts.get(relative) {
        Some(text) => Ok(text.as_bytes().to_vec()),
        None => fs::read(root.join(relative)),
    }
}

/// The last lines of what a program printed.
fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// Copies the files of the project that version control would keep, and
/// writes `texts` over them. The paths of the files placed.
fn copy_project(root: &Path, into: &Path, texts: &Overlays) -> io::Result<BTreeSet<PathBuf>> {
    let mut placed = BTreeSet::new();
    let walk = WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for entry in walk {
        let entry = entry.map_err(io::Error::other)?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        place(&into.join(relative), |target| {
            fs::copy(entry.path(), target).map(drop)
        })?;
        placed.insert(relative.to_owned());
    }
    for (path, text) in texts {
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(io::Error::other(format!(
                "`{}` is not a relative, normalized path",
                path.display()
            )));
        }
        place(&into.join(path), |target| fs::write(target, text))?;
        placed.insert(path.clone());
    }
    Ok(placed)
}

/// Creates the directory of `target` and fills the file by `write`.
fn place(target: &Path, write: impl FnOnce(&Path) -> io::Result<()>) -> io::Result<()> {
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    write(target)
}
