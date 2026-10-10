//! Where a SARIF result is: the file its `artifactLocation` names, as a
//! path inside the project.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use super::log::ArtifactLocation;

/// How many `uriBaseId` hops resolving one location may take.
const BASE_DEPTH: usize = 4;

/// The path a location names, absolute or relative to the project root; the
/// tools that write relative paths without a base mean the directory they ran
/// in, which is the root. `.` and `..` segments are folded. `None` when the
/// location has no `uri`.
pub(super) fn path_of(
    location: &ArtifactLocation,
    bases: &BTreeMap<String, ArtifactLocation>,
) -> Option<PathBuf> {
    resolve(location, bases, BASE_DEPTH).map(|path| fold(&path))
}

/// `path` relative to `root`.
pub(super) fn relative(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

/// `text` with its `%XX` escapes decoded; an escape that is not hexadecimal
/// stays as written.
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let escaped = (bytes[at] == b'%')
            .then(|| bytes.get(at + 1..at + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn resolve(
    location: &ArtifactLocation,
    bases: &BTreeMap<String, ArtifactLocation>,
    depth: usize,
) -> Option<PathBuf> {
    let own = file_path(location.uri.as_deref()?);
    if own.is_absolute() {
        return Some(own);
    }
    let base = location
        .uri_base_id
        .as_ref()
        .and_then(|id| bases.get(id))
        .filter(|_| depth > 0)
        .and_then(|base| resolve(base, bases, depth - 1));
    Some(base.map_or(own.clone(), |base| base.join(&own)))
}

/// A `file:` URI as the path it names (`file:///a`, `file://localhost/a` and
/// `file:/a` alike); any other reference is a relative path. A query or a
/// fragment is not part of the path, and percent-escapes are decoded.
fn file_path(uri: &str) -> PathBuf {
    let uri = uri.split(['#', '?']).next().unwrap_or_default();
    let path = match uri.strip_prefix("file:") {
        Some(rest) => match rest.strip_prefix("//") {
            Some(authority) => authority.find('/').map_or("", |at| &authority[at..]),
            None => rest,
        },
        None => uri,
    };
    PathBuf::from(decode(path))
}

/// `path` without `.` segments and with each `..` folded into the segment
/// before it; a `..` that leads a relative path stays, so it names nothing
/// inside the project.
fn fold(path: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.last(), Some(Component::Normal(_))) => {
                out.pop();
            }
            Component::ParentDir if matches!(out.last(), Some(Component::RootDir)) => {}
            other => out.push(other),
        }
    }
    out.iter().collect()
}
