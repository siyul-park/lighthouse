//! The patterns that keep files out of a project: one syntax (`.gitignore`
//! lines, as ripgrep and fd read them) and one matcher for the project's
//! `ignores` and for the `ignores` of a plugin.

use std::path::Path;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::Error;

/// The matcher of `patterns`, in order (a later one wins), rooted at `root`.
/// A pattern that is not a gitignore line is an error.
pub fn matcher(root: &Path, patterns: &[String]) -> Result<Gitignore, Error> {
    let mut builder = GitignoreBuilder::new(root);
    for (at, pattern) in patterns.iter().enumerate() {
        builder
            .add_line(None, pattern)
            .map_err(|e| Error::Config(format!("`ignores[{at}]` `{pattern}`: {e}")))?;
    }
    builder
        .build()
        .map_err(|e| Error::Config(format!("`ignores`: {e}")))
}
