//! The session baseline of the Claude Code hooks: what a file's findings were
//! before the session first touched it, kept per Claude Code `session_id` in
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
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use lighthouse_model::{Diagnostic, hash};
use lighthouse_resource::write_atomic;
use lighthouse_session::{cache_dir, evidence_digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::Result;

/// Under the cache directory: one directory per session.
const DIR: &str = "hooks";
const MAX_SESSION_ID: usize = 128;
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
            path: key(path),
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

/// The `session_id` of the payload, when it is safe to use as a directory name.
pub fn session_id(payload: &Value) -> Option<&str> {
    let id = payload["session_id"].as_str()?;
    let valid = (1..=MAX_SESSION_ID).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    valid.then_some(id)
}

/// Whether the session already has a baseline of `path`.
pub fn exists(root: &Path, session: &str, path: &Path) -> bool {
    file(root, session, &key(path)).exists()
}

/// The session's baseline of `path`; a missing or unreadable one is `None`.
pub fn load(root: &Path, session: &str, path: &Path) -> Option<Baseline> {
    read(&file(root, session, &key(path)))
}

/// Every baseline of the session, sorted by path.
pub fn session(root: &Path, session: &str) -> Vec<Baseline> {
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
            .find(|b| b.path == key(&d.file))
            .is_none_or(|b| {
                b.findings
                    .get(d.fingerprint.as_str())
                    .is_none_or(|digest| *digest != evidence_digest(&d.evidence))
            })
    });
}

fn read(path: &Path) -> Option<Baseline> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn dir(root: &Path, session: &str) -> PathBuf {
    cache_dir(root).join(DIR).join(session)
}

fn file(root: &Path, session: &str, path: &str) -> PathBuf {
    dir(root, session).join(format!("{}.json", hash::short(path, 8)))
}
