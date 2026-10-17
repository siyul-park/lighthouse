//! The commands of `[fix] verify`: the project's own tests, run over a scratch
//! copy of it that holds the fixed texts, so the files themselves stay as
//! they were until the fixes are accepted.

use std::{
    fs, io,
    path::{Component, Path},
    time::Duration,
};

use ignore::WalkBuilder;
use lighthouse_process::{GRACE, Invocation, MAX_OUTPUT};

use crate::Overlays;

/// Lines of the output of a failed command that the notes keep.
const TAIL_LINES: usize = 20;

/// Runs `commands` in turn over a copy of the project at `root` with `texts`
/// in place of the files they name. The first that fails ends the run: its
/// text and the tail of its output are the error.
pub(in crate::fix) fn run_verify(
    root: &Path,
    texts: &Overlays,
    commands: &[Vec<String>],
    limit: Duration,
) -> Result<(), String> {
    let scratch = tempfile::tempdir().map_err(|e| format!("no scratch copy: {e}"))?;
    copy_project(root, scratch.path(), texts)
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

/// The last lines of what a program printed.
fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// Copies the files of the project that version control would keep, and
/// writes `texts` over them.
fn copy_project(root: &Path, into: &Path, texts: &Overlays) -> io::Result<()> {
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
    }
    Ok(())
}

/// Creates the directory of `target` and fills the file by `write`.
fn place(target: &Path, write: impl FnOnce(&Path) -> io::Result<()>) -> io::Result<()> {
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    write(target)
}
