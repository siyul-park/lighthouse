//! The source text of one arrangement of a shape: what a language's files
//! look like when the declarations stand in that order. The text compiles on
//! its own (Go: `go vet` passes; Rust: the crate type-checks).

use super::{Alphabet, Arrangement, Decl, Language, Relation, Shape};

mod go;
mod rust;

/// A rendered file: its path relative to the package directory, and its text.
pub(super) type File = (String, String);

/// A shape read for one language.
pub(super) struct View<'a> {
    pub(super) shape: &'a Shape,
    pub(super) alphabet: Alphabet,
}

impl View<'_> {
    pub(super) fn decl(&self, index: usize) -> Decl {
        self.shape.decls[index]
    }

    /// The targets of the edges of `relation` that leave `from`.
    pub(super) fn targets(&self, from: usize, relation: Relation) -> Vec<usize> {
        self.shape
            .edges
            .iter()
            .filter(|edge| edge.from == from && edge.relation == relation)
            .map(|edge| edge.to)
            .collect()
    }

    /// The type that owns a method.
    pub(super) fn owner(&self, method: usize) -> usize {
        self.targets(method, Relation::Owner)[0]
    }
}

/// The files of `arrangement` of `shape` as package `package` of `language`.
/// Go tests are a file of their own; Rust tests are a module of the source.
pub(super) fn files(
    language: Language,
    alphabet: Alphabet,
    shape: &Shape,
    arrangement: &Arrangement,
    package: &str,
) -> Vec<File> {
    let view = View { shape, alphabet };
    match language {
        Language::Rust => vec![(format!("{package}.rs"), rust::source(&view, arrangement))],
        Language::Go => {
            let mut files = vec![(
                format!("{package}.go"),
                go::source(&view, arrangement, package),
            )];
            if !arrangement.tests.is_empty() {
                files.push((
                    format!("{package}_test.go"),
                    go::tests(&view, arrangement, package),
                ));
            }
            files
        }
    }
}
