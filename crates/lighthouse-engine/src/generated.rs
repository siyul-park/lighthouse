//! Telling generated code apart in the host, language-neutrally: the
//! `.gitattributes` `linguist-generated` attribute (the GitHub standard) and
//! the project's `generated.files`. A provider's own marker (Go's `// Code
//! generated ... DO NOT EDIT.`, `@generated`) arrives with its fragment.

use std::{fs, path::Path};

use globset::{Glob, GlobBuilder, GlobMatcher};

const ATTRIBUTES_FILE: &str = ".gitattributes";
const ATTRIBUTE: &str = "linguist-generated";

/// The `linguist-generated` lines of a `.gitattributes` at the project root,
/// later lines winning. A pattern without a `/` matches at any depth, one
/// with a leading `/` from the root; `linguist-generated=false` and
/// `-linguist-generated` switch it off again. Nested attribute files are not
/// read.
#[derive(Debug, Default)]
pub(crate) struct Attributes {
    rules: Vec<(GlobMatcher, bool)>,
}

impl Attributes {
    /// The attributes of the project at `root`; none when it has no file.
    pub(crate) fn of(root: &Path) -> Self {
        let text = fs::read_to_string(root.join(ATTRIBUTES_FILE)).unwrap_or_default();
        Self::parse(&text)
    }

    pub(crate) fn parse(text: &str) -> Self {
        let rules = text
            .lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                let pattern = parts.next().filter(|p| !p.starts_with('#'))?;
                let on = parts.filter_map(setting).next_back()?;
                Some((matcher(pattern)?, on))
            })
            .collect();
        Self { rules }
    }

    /// Whether the last line that names `path` marks it generated.
    pub(crate) fn is_generated(&self, path: &Path) -> bool {
        self.rules
            .iter()
            .rev()
            .find(|(glob, _)| glob.is_match(path))
            .is_some_and(|(_, on)| *on)
    }
}

/// What an attribute token says about `linguist-generated`, if it names it.
fn setting(token: &str) -> Option<bool> {
    match token {
        ATTRIBUTE => Some(true),
        "-linguist-generated" | "!linguist-generated" => Some(false),
        other => match other.split_once('=')? {
            (ATTRIBUTE, value) => Some(value != "false"),
            _ => None,
        },
    }
}

fn matcher(pattern: &str) -> Option<GlobMatcher> {
    let anchored = pattern.trim_start_matches('/');
    let glob = if pattern.starts_with('/') || anchored.contains('/') {
        GlobBuilder::new(anchored).literal_separator(true).build()
    } else {
        Glob::new(&format!("**/{anchored}"))
    };
    glob.ok().map(|g| g.compile_matcher())
}
