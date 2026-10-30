//! Telling generated code apart in the host, language-neutrally: the
//! `.gitattributes` `linguist-generated` and `linguist-vendored` attributes
//! (the GitHub standard; both leave a file out of Linguist's statistics and
//! diffs, and vendored code is no more the project's to restyle than
//! generated code is) and the project's `generated.files`. A provider's own marker (Go's `// Code
//! generated ... DO NOT EDIT.`, `@generated`) arrives with its fragment.

use std::{fs, path::Path};

use globset::{Glob, GlobBuilder, GlobMatcher};

const ATTRIBUTES_FILE: &str = ".gitattributes";
const ATTRIBUTES: [&str; 2] = ["linguist-generated", "linguist-vendored"];

/// The `linguist-generated` and `linguist-vendored` lines of a `.gitattributes` at the project root,
/// later lines winning. A pattern without a `/` matches at any depth, one
/// with a leading `/` from the root; `linguist-generated=false` and
/// `-linguist-generated` (likewise for `linguist-vendored`) switch it off again. Nested attribute files are not
/// read.
#[derive(Debug, Default)]
pub struct Attributes {
    rules: Vec<(GlobMatcher, bool)>,
}

impl Attributes {
    /// The attributes of the project at `root`, and what the user should know
    /// about them; none when it has no file. A file that cannot be read and a
    /// pattern that is not a glob are notices: the attribute is not applied.
    pub fn new(root: &Path) -> (Self, Vec<String>) {
        let text = match fs::read_to_string(root.join(ATTRIBUTES_FILE)) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), Vec::new());
            }
            Err(e) => {
                return (
                    Self::default(),
                    vec![format!("{ATTRIBUTES_FILE}: not read, {e}")],
                );
            }
        };
        let mut rules = Vec::new();
        let mut notices = Vec::new();
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let Some(pattern) = parts.next().filter(|p| !p.starts_with('#')) else {
                continue;
            };
            let Some(on) = parts.filter_map(setting).next_back() else {
                continue;
            };
            match matcher(pattern) {
                Ok(glob) => rules.push((glob, on)),
                Err(e) => notices.push(format!(
                    "{ATTRIBUTES_FILE}: `{pattern}` is not a usable pattern, {e}"
                )),
            }
        }
        (Self { rules }, notices)
    }

    /// Whether the last line that names `path` marks it generated.
    pub fn is_generated(&self, path: &Path) -> bool {
        self.rules
            .iter()
            .rev()
            .find(|(glob, _)| glob.is_match(path))
            .is_some_and(|(_, on)| *on)
    }
}

/// What an attribute token says about `linguist-generated` or
/// `linguist-vendored`, if it names one.
fn setting(token: &str) -> Option<bool> {
    let name = token.trim_start_matches(['-', '!']);
    if ATTRIBUTES.contains(&name) {
        return Some(name == token);
    }
    let (name, value) = token.split_once('=')?;
    ATTRIBUTES.contains(&name).then_some(value != "false")
}

fn matcher(pattern: &str) -> Result<GlobMatcher, globset::Error> {
    let anchored = pattern.trim_start_matches('/');
    let glob = if pattern.starts_with('/') || anchored.contains('/') {
        GlobBuilder::new(anchored).literal_separator(true).build()
    } else {
        Glob::new(&format!("**/{anchored}"))
    };
    glob.map(|g| g.compile_matcher())
}
