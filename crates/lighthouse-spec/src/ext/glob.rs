//! Module path globs, the vocabulary of layer contracts, as a pattern language
//! exposed like the `regex` extension: `glob.matches(path, pattern)` is
//! whether the path matches the pattern. `*` is any run of characters inside
//! one path segment, `**` is any number of segments (none included, so
//! `a/**` also matches `a`). Paths and patterns may use `::` for `/`, as Rust
//! spells module paths.
//!
//! ```text
//! glob.matches('app/http', 'app/*')   == true
//! glob.matches('app/http/x', 'app/*') == false
//! glob.matches('app', 'app/**')       == true
//! ```

use cel::{
    DeclarationError, Env,
    common::types::{CelBool, CelString},
};

/// Registers the overload on `env`.
pub(crate) fn install(env: &mut Env) -> Result<(), DeclarationError> {
    cel::add_overload!(env, fn matches_glob: (CelString, CelString) -> CelBool,
        name = "glob.matches")
}

/// Whether `path` matches `glob`.
pub fn matches(path: &str, glob: &str) -> bool {
    let path = path.replace("::", "/");
    let glob = glob.replace("::", "/");
    let path: Vec<&str> = path.split('/').collect();
    let glob: Vec<&str> = glob.split('/').collect();
    segments(&path, &glob)
}

fn matches_glob(path: &CelString<'_>, pattern: &CelString<'_>) -> CelBool {
    CelBool::from(matches(path, pattern))
}

/// Whether the segments of a path match the segments of a glob: a table of
/// the suffixes that match, so `**` costs no backtracking.
fn segments(path: &[&str], glob: &[&str]) -> bool {
    let mut matched = vec![vec![false; glob.len() + 1]; path.len() + 1];
    matched[path.len()][glob.len()] = true;
    for i in (0..=path.len()).rev() {
        for j in (0..glob.len()).rev() {
            matched[i][j] = if glob[j] == "**" {
                matched[i][j + 1] || (i < path.len() && matched[i + 1][j])
            } else {
                i < path.len() && within(path[i], glob[j]) && matched[i + 1][j + 1]
            };
        }
    }
    matched[0][0]
}

/// One segment against a pattern whose `*` stands for any run of characters.
fn within(segment: &str, pattern: &str) -> bool {
    let mut parts = pattern.split('*');
    let Some(first) = parts.next() else {
        return segment.is_empty();
    };
    let Some(mut rest) = segment.strip_prefix(first) else {
        return false;
    };
    let tail: Vec<&str> = parts.collect();
    let Some((last, middle)) = tail.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        let Some(at) = rest.find(part) else {
            return false;
        };
        rest = &rest[at + part.len()..];
    }
    rest.ends_with(last)
}
