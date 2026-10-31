//! What a plugin answered to `initialize`, kept between runs: a run that
//! finds no file the plugin claims then never starts its process. The answer
//! is kept per build of the plugin, so a rebuilt plugin is asked again.

use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use lighthouse_model::write_atomic;
use lighthouse_protocol as wire;
use lighthouse_protocol::hash::Hasher;

use crate::Found;

/// Characters of the build key that name the kept file.
const KEY_CHARS: usize = 16;
/// Entries of the plugin directory that identify a build; a directory with
/// more is not read further, and its plugin is asked every time.
const MAX_ENTRIES: usize = 20_000;
/// Directories of a plugin that never change what it answers.
const SKIPPED: [&str; 4] = ["node_modules", "__pycache__", "decisions", ".git"];

/// Where the answer of one build of a plugin is kept.
pub(crate) struct Kept {
    base: PathBuf,
    path: PathBuf,
}

impl Kept {
    /// The place for the answer of `found` under `base`; `None` when the build
    /// cannot be identified (a bare command found on the `PATH`, an unreadable
    /// binary, a directory too large to stamp).
    pub(crate) fn of(base: &Path, found: &Found) -> Option<Self> {
        let key = build_key(found)?;
        let path = base
            .join(found.id())
            .join(format!("initialize-{}.json", &key[..KEY_CHARS]));
        Some(Self {
            base: base.to_owned(),
            path,
        })
    }

    /// The kept answer, when there is one that reads.
    pub(crate) fn read(&self) -> Option<wire::InitializeResult> {
        let text = fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Keeps `result`. The answer is only a shortcut: when it cannot be kept
    /// the plugin is asked again next time, so a failure is not reported.
    pub(crate) fn write(&self, result: &wire::InitializeResult) {
        let Ok(text) = serde_json::to_string(result) else {
            return;
        };
        let Some(dir) = self.path.parent() else {
            return;
        };
        let ignore = self.base.join(".gitignore");
        let made = fs::create_dir_all(dir).and_then(|()| {
            if ignore.exists() {
                Ok(())
            } else {
                fs::write(&ignore, "*\n")
            }
        });
        if made.is_ok() {
            let _ = write_atomic(&self.path, &text);
        }
    }
}

/// A key that changes with the binary the plugin runs, the files beside it
/// (a plugin that is a script runs files other than the one it is started by),
/// its manifest version and the protocol and host that read its answer.
fn build_key(found: &Found) -> Option<String> {
    let mut hash = Hasher::new();
    hash.update(&fs::read(found.command_path()?).ok()?);
    for part in [
        found.resource.spec.version.as_str(),
        wire::VERSION,
        env!("CARGO_PKG_VERSION"),
    ] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    let mut seen = 0;
    stamp(&found.dir, &found.dir, &mut hash, &mut seen)?;
    Some(hash.hex())
}

/// Feeds `hash` the relative path, size and modification time of every file
/// under `dir`, in path order. `None` past [`MAX_ENTRIES`] or on a directory
/// that cannot be read.
fn stamp(root: &Path, dir: &Path, hash: &mut Hasher, seen: &mut usize) -> Option<()> {
    let mut entries: Vec<_> = fs::read_dir(dir).ok()?.collect::<Result<_, _>>().ok()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        *seen += 1;
        if *seen > MAX_ENTRIES {
            return None;
        }
        let name = entry.file_name();
        let meta = fs::symlink_metadata(entry.path()).ok()?;
        if meta.is_dir() {
            if !SKIPPED.iter().any(|skipped| name == *skipped) {
                stamp(root, &entry.path(), hash, seen)?;
            }
            continue;
        }
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos());
        hash.update(
            entry
                .path()
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .as_bytes(),
        );
        hash.update([0]);
        hash.update(meta.len().to_le_bytes());
        hash.update(modified.to_le_bytes());
    }
    Some(())
}
