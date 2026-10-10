//! Module path globs, the vocabulary of layer contracts: `*` is any run of
//! characters inside one path segment, `**` is any number of segments (none
//! included, so `a/**` also matches `a`). Paths and globs may use `::` for
//! `/`, as Rust spells module paths.

/// Whether `path` matches `glob`.
pub(crate) fn matches(path: &str, glob: &str) -> bool {
    let path = path.replace("::", "/");
    let glob = glob.replace("::", "/");
    let path: Vec<&str> = path.split('/').collect();
    let glob: Vec<&str> = glob.split('/').collect();
    segments(&path, &glob)
}

/// The index of the first layer, top to bottom, with a glob that matches
/// `module`; -1 when no layer has one.
pub(crate) fn layer_of<'a>(
    module: &str,
    layers: impl IntoIterator<Item = impl IntoIterator<Item = &'a str>>,
) -> i64 {
    layers
        .into_iter()
        .position(|globs| globs.into_iter().any(|glob| matches(module, glob)))
        .and_then(|at| i64::try_from(at).ok())
        .unwrap_or(-1)
}

fn segments(path: &[&str], glob: &[&str]) -> bool {
    match glob.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments(&path[skip..], rest)),
        Some((pattern, rest)) => path
            .split_first()
            .is_some_and(|(segment, tail)| within(segment, pattern) && segments(tail, rest)),
    }
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
