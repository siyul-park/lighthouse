//! The regular expressions that mark a file generated when a comment at its
//! head matches one, set by the host and applied to the fragments of a file.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use regex::{RegexSet, RegexSetBuilder};

use super::{Comment, File, Fragment, Symbol};

/// The regular expressions that mark a file generated when a head comment
/// matches one.
#[derive(Debug)]
pub struct Markers {
    patterns: Vec<String>,
    set: RegexSet,
}

impl Markers {
    /// The markers of these patterns; one that is no regular expression is
    /// an error.
    pub fn new(patterns: Vec<String>) -> Result<Self, regex::Error> {
        Ok(Self {
            set: RegexSetBuilder::new(&patterns).build()?,
            patterns,
        })
    }

    /// The markers as they were given.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// The markers of these patterns, compiled once for the process: the same
    /// patterns are the same markers.
    pub fn shared(patterns: &[String]) -> Result<Arc<Self>, regex::Error> {
        static COMPILED: OnceLock<Mutex<HashMap<Vec<String>, Arc<Markers>>>> = OnceLock::new();
        let mut compiled = COMPILED
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = compiled.get(patterns) {
            return Ok(found.clone());
        }
        let made = Arc::new(Self::new(patterns.to_vec())?);
        compiled.insert(patterns.to_vec(), made.clone());
        Ok(made)
    }

    /// Marks the files of `fragment` generated whose head comments carry a
    /// marker. The head of a file is its comments that end before its first
    /// symbol starts, or all of them when it has none.
    pub fn mark(&self, fragment: &mut Fragment) {
        if self.set.is_empty() {
            return;
        }
        let marked: Vec<PathBuf> = fragment
            .files
            .iter()
            .filter(|file| !file.generated && self.carries(file, fragment))
            .map(|file| file.path.clone())
            .collect();
        for file in &mut fragment.files {
            file.generated = file.generated || marked.contains(&file.path);
        }
    }

    fn carries(&self, file: &File, fragment: &Fragment) -> bool {
        let first = fragment
            .symbols
            .iter()
            .filter(|s| s.file == file.path)
            .map(start)
            .min();
        fragment
            .comments
            .iter()
            .filter(|c| c.file == file.path && first.is_none_or(|first| ends_before(c, first)))
            .any(|c| self.set.is_match(&c.text))
    }
}

fn start(symbol: &Symbol) -> (u32, u32) {
    (symbol.span.start.line, symbol.span.start.col)
}

fn ends_before(comment: &Comment, at: (u32, u32)) -> bool {
    (comment.span.end.line, comment.span.end.col) <= at
}
