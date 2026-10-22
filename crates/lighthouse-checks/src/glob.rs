//! Paths as text.

use std::path::Path;

/// The path as text with `/` separators, whatever the platform spells.
pub(crate) fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
