//! The scratch copy of the project, which the programs of `[fix] verify`, a
//! command fix and an agent that edits run in, so that the files themselves
//! stay as they were until the fixes are accepted. The copy holds the fixed
//! texts of the run in place of the files they name and is a repository with
//! one commit of that state, so that a tool finds its root, `git diff` works
//! and an agent that refuses to run outside a repository runs. It is deleted
//! when it is dropped.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use ignore::WalkBuilder;
use lighthouse_process::{Environment, Home, Job, Policy, Ran, copy_project};
use lighthouse_spec::{Output, Point, Program};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use crate::Overlays;

/// Lines of the output of a failed command that the notes keep.
const TAIL_LINES: usize = 20;

/// Changes to files that are not edits of a text that an agent made, after
/// which looking for more is pointless: any of them rejects its work.
const MAX_OPAQUE: usize = 32;

/// The directory of the repository, which is no part of the project.
const REPOSITORY: &str = ".git";

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
    /// would keep, with `texts` written over the files they name, as a
    /// repository of one commit. The files are cloned where the file system
    /// can, so the copy costs the directory entries and not the bytes.
    pub(in crate::fix) fn new(root: &Path, texts: &Overlays) -> io::Result<Self> {
        let dir = tempfile::tempdir()?;
        let copied = copy_project(root, dir.path(), texts)?;
        commit(dir.path())?;
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
            .filter_entry(|entry| entry.file_name() != REPOSITORY)
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

/// Runs `programs` in turn over a copy of the project at `root` with `texts`
/// in place of the files they name. The first that fails ends the run: its
/// text and the tail of its output are the error.
pub(in crate::fix) fn run_verify(
    root: &Path,
    texts: &Overlays,
    programs: &[Program],
    policy: &Policy,
) -> Result<(), String> {
    let scratch = Scratch::new(root, texts)
        .map_err(|e| format!("the scratch copy of the project failed: {e}"))?;
    for program in programs {
        let shown = program.shown();
        let own = program.policy(policy);
        let mut profile = own.profile(root, scratch.path()).caching();
        profile.writes.push(scratch.path().to_owned());
        let environment = Environment::new(&program.env, Vec::new(), Home::User)
            .map_err(|reason| format!("`{shown}`: {reason}"))?;
        let ran = Job {
            command: &program.command,
            dir: scratch.path(),
            stdin: &[],
            environment: &environment,
            timeout: program.timeout(Point::Verify),
            policy: &own,
            profile: &profile,
        }
        .run()
        .map_err(|reason| format!("`{shown}`: {reason}"))?;
        let success = program.success(Point::Verify);
        let ended = ended(&ran, &success);
        let failure = match program.output(Point::Verify) {
            Output::Lines => ended.or_else(|| said(&ran)),
            _ => ended,
        };
        if let Some(why) = failure {
            return Err(format!("`{shown}` {why}"));
        }
    }
    Ok(())
}

/// How a program that did not exit with a code of `success` ended, with the
/// tail of what it said.
fn ended(ran: &Ran, success: &[i32]) -> Option<String> {
    if ran.code.is_some_and(|code| success.contains(&code)) {
        return None;
    }
    let how = ran.code.map_or("was ended by a signal".to_owned(), |c| {
        format!("exited {c}")
    });
    let (stderr, stdout) = (tail(&ran.stderr), tail(&ran.stdout));
    let output = if stderr.is_empty() { stdout } else { stderr };
    Some(format!("{how}:\n{output}"))
}

/// The findings a program of `output: lines` printed, as the reason it
/// fails.
fn said(ran: &Ran) -> Option<String> {
    let text = tail(&ran.stdout);
    (!text.trim().is_empty()).then(|| format!("found:\n{text}"))
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

/// Makes the directory a repository with one commit of everything in it. The
/// settings of the user and of the machine are not read, so what the commit
/// holds does not depend on them.
fn commit(dir: &Path) -> io::Result<()> {
    let git = |args: &[&str]| -> io::Result<()> {
        let ran = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| io::Error::other(format!("git cannot be run: {e}")))?;
        if ran.status.success() {
            return Ok(());
        }
        let said = String::from_utf8_lossy(&ran.stderr);
        Err(io::Error::other(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            said.trim()
        )))
    };
    git(&["init", "--quiet"])?;
    git(&["add", "--all", "--force"])?;
    git(&[
        "-c",
        "user.name=lighthouse",
        "-c",
        "user.email=lighthouse@localhost",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.hooksPath=/dev/null",
        "commit",
        "--quiet",
        "--allow-empty",
        "--no-verify",
        "--message",
        "base",
    ])
}
