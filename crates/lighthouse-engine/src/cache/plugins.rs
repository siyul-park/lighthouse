//! What plugins keep in the cache directory: `<plugin>/initialize-<build>.json`,
//! written by the host's plugin loader (`lighthouse-rpc`). Before the graph
//! kept what providers return, a directory per build of a provider held
//! fragments; nothing reads or writes them now.

use std::{fs, path::Path};

use super::HOOKS;

/// The prefix and suffix of the file a plugin's answer is kept in.
const KEPT: (&str, &str) = ("initialize-", ".json");

/// Deletes from each plugin's directory everything but its newest kept
/// answer. A run that asks for an answer that is gone asks the plugin again.
pub(super) fn forget_leftovers(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for plugin in entries.flatten().filter(|e| e.file_name() != HOOKS) {
        let Ok(inside) = fs::read_dir(plugin.path()) else {
            continue;
        };
        let (kept, other): (Vec<_>, Vec<_>) = inside.flatten().partition(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(KEPT.0) && name.ends_with(KEPT.1)
        });
        let newest = kept
            .iter()
            .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
            .map(fs::DirEntry::path);
        let stale = kept.iter().chain(&other).map(fs::DirEntry::path);
        for path in stale.filter(|path| Some(path) != newest.as_ref()) {
            if path.is_dir() {
                let _ = fs::remove_dir_all(&path);
            } else {
                let _ = fs::remove_file(&path);
            }
        }
    }
}
