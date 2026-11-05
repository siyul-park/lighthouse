//! Telling third-party and generated code apart in the host, language-neutrally:
//! the `.gitattributes` `linguist-vendored` and `linguist-generated`
//! attributes, which are two things (the GitHub Linguist standard): vendored
//! code is not the project's, so the project leaves it out; generated code is
//! the project's, but decisions skip it. A provider's own marker (Go's `// Code
//! generated ... DO NOT EDIT.`, `@generated`) arrives with its fragment.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobBuilder, GlobMatcher};

const ATTRIBUTES_FILE: &str = ".gitattributes";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attribute {
    Generated,
    Vendored,
}

/// One line of a `.gitattributes` file about one attribute.
#[derive(Debug)]
struct Rule {
    /// The directory of the file the line is in; the pattern is relative to it.
    base: PathBuf,
    glob: GlobMatcher,
    attribute: Attribute,
    on: bool,
}

/// The `linguist-generated` and `linguist-vendored` lines of the
/// `.gitattributes` files of a project, with git's semantics: a file's
/// patterns are relative to its directory (a pattern without a `/` matches at
/// any depth below it, one with a `/` from it), a file deeper in the tree
/// overrides a shallower one, later lines win, and each attribute is decided
/// on its own. `linguist-x=false`, `-linguist-x` and `!linguist-x` switch the
/// attribute off again.
#[derive(Debug, Default)]
pub struct Attributes {
    rules: Vec<Rule>,
}

impl Attributes {
    /// Whether the project-relative `file` is a `.gitattributes`.
    pub fn is_file(file: &Path) -> bool {
        file.file_name().is_some_and(|name| name == ATTRIBUTES_FILE)
    }

    /// The attributes of the `.gitattributes` files at the project-relative
    /// paths, each with its text (`None` when it cannot be read), and what
    /// the user should know about them. A file that cannot be read and a
    /// pattern that is not a glob are notices: the attribute is not applied.
    pub fn from_texts(mut files: Vec<(PathBuf, Option<String>)>) -> (Self, Vec<String>) {
        files.sort_by_key(|(f, _)| (f.components().count(), f.clone()));
        let mut rules = Vec::new();
        let mut notices = Vec::new();
        for (file, text) in files {
            let base = file.parent().unwrap_or(Path::new("")).to_owned();
            let Some(text) = text else {
                notices.push(format!("{}: not read", file.display()));
                continue;
            };
            for line in text.lines() {
                let mut parts = line.split_whitespace();
                let Some(pattern) = parts.next().filter(|p| !p.starts_with('#')) else {
                    continue;
                };
                let settings: Vec<(Attribute, bool)> = parts.filter_map(setting).collect();
                if settings.is_empty() {
                    continue;
                }
                match matcher(pattern) {
                    Ok(glob) => {
                        for (attribute, on) in settings {
                            rules.push(Rule {
                                base: base.clone(),
                                glob: glob.clone(),
                                attribute,
                                on,
                            });
                        }
                    }
                    Err(e) => notices.push(format!(
                        "{}: `{pattern}` is not a usable pattern, {e}",
                        file.display()
                    )),
                }
            }
        }
        (Self { rules }, notices)
    }

    /// Whether the last line that names `path` marks it `linguist-generated`.
    pub fn is_generated(&self, path: &Path) -> bool {
        self.decided(path, Attribute::Generated)
    }

    /// Whether the last line that names `path` marks it `linguist-vendored`.
    pub fn is_vendored(&self, path: &Path) -> bool {
        self.decided(path, Attribute::Vendored)
    }

    fn decided(&self, path: &Path, attribute: Attribute) -> bool {
        self.rules
            .iter()
            .rev()
            .filter(|r| r.attribute == attribute)
            .find(|r| {
                path.strip_prefix(&r.base)
                    .is_ok_and(|rel| r.glob.is_match(rel))
            })
            .is_some_and(|r| r.on)
    }
}

/// What an attribute token says about `linguist-generated` or
/// `linguist-vendored`, if it names one.
fn setting(token: &str) -> Option<(Attribute, bool)> {
    let name = token.trim_start_matches(['-', '!']);
    if let Some(attribute) = named(name) {
        return Some((attribute, name == token));
    }
    let (name, value) = token.split_once('=')?;
    Some((named(name)?, value != "false"))
}

fn named(name: &str) -> Option<Attribute> {
    match name {
        "linguist-generated" => Some(Attribute::Generated),
        "linguist-vendored" => Some(Attribute::Vendored),
        _ => None,
    }
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
