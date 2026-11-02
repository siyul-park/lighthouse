//! Telling generated code apart in the host: the files a project's `generated`
//! names, and the markers a generator leaves in the head of what it writes.

use regex::{RegexSet, RegexSetBuilder};

use crate::Error;

/// The markers as one set, searched unanchored in a comment's text. A marker
/// that is not a regular expression is an error.
pub fn markers(patterns: &[String]) -> Result<RegexSet, Error> {
    RegexSetBuilder::new(patterns)
        .build()
        .map_err(|e| Error::Config(format!("`generated.markers`: {e}")))
}
