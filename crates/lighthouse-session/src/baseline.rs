//! The session baseline of the agent hooks: what a file's findings were
//! before the session first touched it, kept per agent `session_id` in
//! `.lighthouse/cache/hooks/<session_id>/<hash of path>.json`. A hook reports
//! only the findings that are `new` (fingerprint absent from the baseline) or
//! `updated` (same fingerprint, different evidence digest) against it, the SARIF
//! `baselineState`s; `unchanged` ones were there before the session.
//!
//! One file per touched path, so parallel tool calls never race on a shared
//! file. A session id that is not `[A-Za-z0-9_-]{1,128}` is no session: it is
//! never joined to a path.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use lighthouse_engine::{SKIPPED_DIRS, cache};
use lighthouse_model::{Diagnostic, evidence_digest};
use lighthouse_protocol::hash;
use lighthouse_resource::{STATE_DIR, slashed, write_atomic};
use serde::{Deserialize, Serialize};

use crate::Result;

/// Under the cache directory: one directory per session.
const DIR: &str = "hooks";
const MAX_SESSION_ID: usize = 128;
/// Bytes read to tell text from binary.
const SNIFF: usize = 8192;
/// Session directories older than this are deleted whenever one is written.
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The findings of one file before the session's first edit of it.
#[derive(Debug, Serialize, Deserialize)]
pub struct Baseline {
    /// Project-relative, `/`-separated.
    path: String,
    /// Fingerprint to evidence digest.
    findings: BTreeMap<String, String>,
}

impl Baseline {
    /// The baseline of `path` (project-relative) that has `findings`; none for
    /// a file that does not exist yet.
    pub fn of(path: &Path, findings: &[Diagnostic]) -> Self {
        Self {
            path: slashed(path),
            findings: findings
                .iter()
                .map(|d| {
                    (
                        d.fingerprint.as_str().to_owned(),
                        evidence_digest(&d.evidence),
                    )
                })
                .collect(),
        }
    }

    /// The file this baseline is of, project-relative.
    pub fn path(&self) -> &Path {
        Path::new(&self.path)
    }
}

/// The `session_id`, when it is safe to use as a directory name.
pub fn session_id(id: Option<&str>) -> Option<&str> {
    let id = id?;
    let valid = (1..=MAX_SESSION_ID).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    valid.then_some(id)
}

/// Whether the session already has a baseline of `path`.
pub fn exists(root: &Path, session: &str, path: &Path) -> bool {
    file(root, session, &slashed(path)).exists()
}

/// The session's baseline of `path`; a missing or unreadable one is `None`.
pub fn load(root: &Path, session: &str, path: &Path) -> Option<Baseline> {
    read(&file(root, session, &slashed(path)))
}

/// Every baseline of the session, sorted by path.
pub fn load_all(root: &Path, session: &str) -> Vec<Baseline> {
    let Ok(entries) = fs::read_dir(dir(root, session)) else {
        return Vec::new();
    };
    let mut all: Vec<Baseline> = entries.flatten().filter_map(|e| read(&e.path())).collect();
    all.sort_by(|a, b| a.path.cmp(&b.path));
    all
}

/// Writes the baseline and deletes the session directories that are too old.
pub fn record(root: &Path, session: &str, baseline: &Baseline) -> Result<()> {
    let own = dir(root, session);
    fs::create_dir_all(&own)?;
    write_atomic(
        &file(root, session, &baseline.path),
        &serde_json::to_string(baseline)?,
    )?;
    // Pruning is housekeeping: a directory that cannot be deleted stays for the
    // next write and never fails the hook.
    for old in fs::read_dir(own.parent().unwrap_or(&own))?.flatten() {
        let stale = old
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age > MAX_AGE);
        if stale && old.path() != own {
            let _ = fs::remove_dir_all(old.path());
        }
    }
    Ok(())
}

/// Keeps the `new` and `updated` findings against the baseline of their file.
/// A finding in a file with no baseline is kept: nothing says it was there.
pub fn introduced(findings: &mut Vec<Diagnostic>, baselines: &[Baseline]) {
    findings.retain(|d| {
        baselines
            .iter()
            .find(|b| b.path == slashed(&d.file))
            .is_none_or(|b| {
                b.findings
                    .get(d.fingerprint.as_str())
                    .is_none_or(|digest| *digest != evidence_digest(&d.evidence))
            })
    });
}

/// The fingerprints the `baselines` hold that `findings`, from a complete
/// check of the baselines' files, no longer do: what the session resolved.
pub fn gone(baselines: &[Baseline], findings: &[Diagnostic]) -> Vec<String> {
    baselines
        .iter()
        .flat_map(|b| b.findings.keys())
        .filter(|fp| {
            findings
                .iter()
                .all(|d| d.fingerprint.as_str() != fp.as_str())
        })
        .cloned()
        .collect()
}

/// The project-relative path of `path`, an absolute path an editing tool
/// touches, when it is worth checking: it lies inside the project (`root`,
/// canonical), is not in a directory Lighthouse never reads, and is text when
/// it exists. A file the tool is about to create counts. Everything else is a
/// cheap `None`, before any plugin starts.
pub fn editable(path: &Path, root: &Path) -> Option<PathBuf> {
    let absolute = resolved(path)?;
    let relative = absolute.strip_prefix(root).ok()?;
    let skipped = relative
        .components()
        .any(|c| c.as_os_str() == STATE_DIR || SKIPPED_DIRS.iter().any(|d| c.as_os_str() == *d));
    (!skipped && (!absolute.exists() || is_text(&absolute))).then(|| relative.to_owned())
}

fn read(path: &Path) -> Option<Baseline> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn dir(root: &Path, session: &str) -> PathBuf {
    cache::dir(root).join(DIR).join(session)
}

fn file(root: &Path, session: &str, path: &str) -> PathBuf {
    dir(root, session).join(format!("{}.json", hash::short(path, 8)))
}

/// `path` made canonical as far as it exists, so that a file about to be
/// created resolves like one that is there; `..` in the part that does not
/// exist is refused.
fn resolved(path: &Path) -> Option<PathBuf> {
    let mut missing = PathBuf::new();
    let mut here = path;
    loop {
        if let Ok(found) = here.canonicalize() {
            return Some(found.join(missing));
        }
        missing = Path::new(here.file_name()?).join(missing);
        here = here.parent()?;
    }
}

fn is_text(path: &Path) -> bool {
    let mut head = Vec::with_capacity(SNIFF);
    let Ok(file) = File::open(path) else {
        return false;
    };
    file.take(SNIFF as u64)
        .read_to_end(&mut head)
        .is_ok_and(|_| !head.contains(&0))
}
