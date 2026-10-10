//! Where a SARIF result is: the file its `artifactLocation` names, as a
//! path inside the project.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use super::log::ArtifactLocation;

/// How many `uriBaseId` hops resolving one location may take.
const BASE_DEPTH: usize = 4;

/// The path a location names, absolute or relative to the project root; the
/// tools that write relative paths without a base mean the directory they ran
/// in, which is the root. `None` when the location has no `uri`.
pub(super) fn path_of(
    location: &ArtifactLocation,
    bases: &BTreeMap<String, ArtifactLocation>,
) -> Option<PathBuf> {
    resolve(location, bases, BASE_DEPTH)
}

/// `path` relative to `root`, without `.` components.
pub(super) fn relative(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
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

/// A `file://` URI as the path it names; any other reference is a relative
/// path, with its percent-escapes decoded.
fn file_path(uri: &str) -> PathBuf {
    let rest = uri
        .strip_prefix("file://")
        .map(|r| r.strip_prefix("localhost").unwrap_or(r));
    PathBuf::from(decode(rest.unwrap_or(uri)))
}

fn decode(text: &str) -> String {
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
