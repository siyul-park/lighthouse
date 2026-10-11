//! The result cache of a project: where it lives, how big it may get, and
//! how it is cleaned.

use std::path::{Path, PathBuf};

use crate::Result;

/// The environment variable that sets the size limit of the cache, in MB.
pub const LIMIT_VAR: &str = "LIGHTHOUSE_CACHE_LIMIT_MB";

/// Where the cache of the project at `root` lives: `<root>/.lighthouse/cache`.
pub fn cache_dir(root: &Path) -> PathBuf {
    root.join(".lighthouse").join(lighthouse_cache::DIR)
}

/// The size limit of the cache in bytes: `LIGHTHOUSE_CACHE_LIMIT_MB`, else
/// 256 MB.
pub(crate) fn cache_limit() -> u64 {
    std::env::var(LIMIT_VAR)
        .ok()
        .and_then(|mb| mb.trim().parse::<u64>().ok())
        .map_or(lighthouse_cache::DEFAULT_LIMIT, |mb| {
            mb.saturating_mul(1 << 20)
        })
}

/// Deletes the cache of the project at `root` entirely, including whatever the
/// language providers keep in it; returns whether there was one.
pub fn clean_cache(root: &Path) -> Result<bool> {
    Ok(lighthouse_cache::clean(&cache_dir(root))?)
}
