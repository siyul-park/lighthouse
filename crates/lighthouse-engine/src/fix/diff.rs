use std::path::Path;

use similar::TextDiff;

/// The unified diff from `before` to `after` of one file; empty when equal.
pub fn unified_diff(path: &Path, before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }
    let name = path.to_string_lossy().replace('\\', "/");
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string()
}
