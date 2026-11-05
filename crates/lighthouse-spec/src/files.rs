//! The files of a project, decided once: the walk every command shares.
//!
//! A file is part of the project unless, in this order, it is in `.git/` or in
//! the state directory (but for `.lighthouse/decisions/`, the only layout of
//! Lighthouse's own); a `.gitignore` (nested ones, `.git/info/exclude`) keeps
//! it out; one of the `ignores` patterns (the listed plugins' conventions,
//! then the extended projects' and the project's own) keeps it out; or
//! `.gitattributes` marks it `linguist-vendored`. Nothing else is special:
//! hidden directories are walked.

use std::path::{Component, Path, PathBuf};

use crate::{Attributes, Disk, Error, Source, Walked, ignores::matcher as ignore_matcher};
use ignore::{WalkBuilder, gitignore::Gitignore};
use lighthouse_model::STATE_DIR;

/// The directory of a repository, which is no part of a project.
const REPOSITORY: &str = ".git";

/// How to list the files of one project root.
#[derive(Clone)]
pub struct ProjectFiles {
    root: PathBuf,
    patterns: Vec<String>,
    ignores: Gitignore,
}

/// The files of a project and what was learned listing them.
#[derive(Default)]
pub struct Listing {
    /// Project-relative paths, sorted.
    pub files: Vec<PathBuf>,
    /// The `.gitattributes` of the project.
    pub attributes: Attributes,
    /// What the user should know: attribute files that could not be read.
    pub notices: Vec<String>,
    /// Entries the walk could not read.
    pub errors: Vec<String>,
}

impl ProjectFiles {
    /// The files under `root` that `patterns` (the project's `ignores`, in
    /// the order they apply) leave in the project.
    pub fn new(root: &Path, patterns: &[String]) -> Result<Self, Error> {
        Ok(Self {
            root: root.to_owned(),
            patterns: patterns.to_vec(),
            ignores: ignore_matcher(root, patterns)?,
        })
    }

    /// The same rules over another root: a copy of the project.
    pub fn at(&self, root: &Path) -> Self {
        Self::new(root, &self.patterns).expect("the patterns were valid for the first root")
    }

    /// The project root the paths are relative to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every file of the project and its `lstat`, from what `source` holds:
    /// the directories it lists and the text of the ignore files it gives.
    pub fn walk(&self, source: &mut impl Source) -> Walked {
        crate::walk::walk(self, source)
    }

    /// Every file of the project, sorted: a walk of the disk.
    pub fn list(&self) -> Listing {
        self.walk(&mut Disk(&self.root)).listing
    }

    /// Whether the walk enters the entry at `rel`, a project-relative path.
    pub(crate) fn enters(&self, rel: &Path, is_dir: bool) -> bool {
        if rel.as_os_str().is_empty() {
            return true;
        }
        let names: Vec<&std::ffi::OsStr> = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect();
        if names.iter().any(|n| *n == REPOSITORY) {
            return false;
        }
        if names.first().is_some_and(|n| *n == STATE_DIR)
            && names.get(1).is_some_and(|n| *n != "decisions")
        {
            return false;
        }
        is_dir && !self.ignores.matched(rel, true).is_ignore()
            || !is_dir
                && !self
                    .ignores
                    .matched_path_or_any_parents(rel, false)
                    .is_ignore()
    }

    /// Whether the file or directory at `rel` is in the project, judged from
    /// the paths alone: for a path that need not exist yet. The `.gitignore`
    /// files from the root down to it decide, deepest first.
    pub fn contains(&self, rel: &Path) -> bool {
        if !self.enters(rel, false) {
            return false;
        }
        for dir in rel.ancestors().skip(1) {
            let (ignore, _) = Gitignore::new(self.root.join(dir).join(".gitignore"));
            if let m @ (ignore::Match::Ignore(_) | ignore::Match::Whitelist(_)) =
                ignore.matched_path_or_any_parents(self.root.join(rel), false)
            {
                return !m.is_ignore();
            }
        }
        true
    }

    /// Every file of the project, sorted, by the `ignore` crate's own walker:
    /// the reference that [`ProjectFiles::walk`] is tested against, which is
    /// what a run uses.
    #[doc(hidden)]
    pub fn walker_list(&self) -> Listing {
        let this = self.clone();
        let root = self.root.clone();
        let walk = WalkBuilder::new(&self.root)
            .require_git(false)
            .hidden(false)
            .parents(false)
            .git_global(false)
            .ignore(false)
            .filter_entry(move |entry| {
                let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
                entry
                    .path()
                    .strip_prefix(&root)
                    .is_ok_and(|rel| this.enters(rel, is_dir))
            })
            .build();
        let mut listing = Listing::default();
        for entry in walk {
            match entry {
                Ok(entry) if entry.file_type().is_some_and(|t| t.is_file()) => {
                    if let Ok(rel) = entry.path().strip_prefix(&self.root) {
                        listing.files.push(rel.to_owned());
                    }
                }
                Ok(_) => {}
                Err(e) => listing.errors.push(e.to_string()),
            }
        }
        listing.files.sort();
        let texts = listing
            .files
            .iter()
            .filter(|f| Attributes::is_file(f))
            .map(|f| (f.clone(), std::fs::read_to_string(self.root.join(f)).ok()))
            .collect();
        let (attributes, notices) = Attributes::from_texts(texts);
        listing.notices = notices;
        listing.files.retain(|f| !attributes.is_vendored(f));
        listing.attributes = attributes;
        listing
    }
}
