//! Which files are generated code, decided once by the host. A file is
//! generated when a `generated.files` pattern names it, `.gitattributes`
//! marks it `linguist-generated`, or a `generated.markers` regular expression
//! is found in a comment at its head. The patterns and the markers are the
//! listed plugins', then the extended projects' and the project's own; no
//! marker belongs to a language.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use ignore::gitignore::Gitignore;
use lighthouse_model::ucm::{Comment, File, Fragment, Symbol};
use lighthouse_plugin::Registry;
use lighthouse_spec::{Config, generated_markers, ignore_matcher};
use regex::RegexSet;

use crate::Error;

/// The files and the markers that tell generated code apart.
pub(crate) struct Generated {
    files: Gitignore,
    pub markers: Markers,
}

/// The regular expressions that mark a file generated when a head comment
/// matches one.
#[derive(Debug)]
pub(crate) struct Markers {
    patterns: Vec<String>,
    set: RegexSet,
}

impl Generated {
    /// What `registry` (for its plugins `config` lists) and `config` say.
    pub(crate) fn new(root: &Path, registry: &Registry, config: &Config) -> Result<Self, Error> {
        let listed = registry.generated().filter(|(id, _)| config.lists(id));
        let mut files = Vec::new();
        let mut markers = Vec::new();
        for (_, plugin) in listed {
            files.extend(plugin.files.iter().cloned());
            markers.extend(plugin.markers.iter().cloned());
        }
        files.extend(config.generated_files().iter().cloned());
        markers.extend(config.generated_markers().iter().cloned());
        Ok(Self {
            files: ignore_matcher(root, &files)?,
            markers: Markers::new(markers)?,
        })
    }

    /// Whether a `files` pattern names the project-relative `path`.
    pub(crate) fn names(&self, path: &Path) -> bool {
        self.files
            .matched_path_or_any_parents(path, false)
            .is_ignore()
    }
}

impl Markers {
    pub(crate) fn new(patterns: Vec<String>) -> Result<Self, Error> {
        Ok(Self {
            set: generated_markers(&patterns)?,
            patterns,
        })
    }

    /// The markers as they were given.
    pub(crate) fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// The markers of these patterns, compiled once for the process: the same
    /// patterns are the same markers.
    pub(crate) fn shared(patterns: &[String]) -> Result<Arc<Self>, Error> {
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
    pub(crate) fn mark(&self, fragment: &mut Fragment) {
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

#[cfg(test)]
mod tests {
    use lighthouse_model::{
        Position, Span,
        ucm::{Comment, File, Fragment, Symbol, SymbolId, SymbolKind, Visibility},
    };
    use lighthouse_plugin::Registry;
    use lighthouse_spec::Config;

    use super::*;

    fn span(from: u32, to: u32) -> Span {
        let at = |line| Position { line, col: 1 };
        Span {
            start: at(from),
            end: at(to),
        }
    }

    fn comment(at: &str, lines: (u32, u32), text: &str) -> Comment {
        Comment {
            file: at.into(),
            span: span(lines.0, lines.1),
            text: text.to_owned(),
            attached_to: None,
        }
    }

    /// A file of one function starting at line 5, with `comments`.
    fn fragment(path: &str, comments: Vec<Comment>, symbols: bool) -> Fragment {
        let function = Symbol::new(
            SymbolId::new("m", &[], "f", SymbolKind::Function),
            SymbolKind::Function,
            Visibility::Public,
            path.into(),
            span(5, 6),
            "f".to_owned(),
        );
        Fragment {
            files: vec![File {
                path: path.into(),
                lang: "x".to_owned(),
                hash: String::new(),
                generated: false,
                test: false,
                facts: Default::default(),
            }],
            symbols: if symbols { vec![function] } else { Vec::new() },
            comments,
            ..Fragment::default()
        }
    }

    fn generated(toml: &str) -> Generated {
        let config = Config::parse_inline(toml).unwrap();
        Generated::new(Path::new("/p"), &Registry::default(), &config).unwrap()
    }

    fn marked(generated: &Generated, mut fragment: Fragment) -> bool {
        generated.markers.mark(&mut fragment);
        fragment.files[0].generated
    }

    const MARKER: &str = "[generated]\nmarkers = ['@generated\\b']\n";

    #[test]
    fn a_marker_in_a_head_comment_marks_the_file_and_the_same_text_later_does_not() {
        let found = generated(MARKER);

        let head = fragment(
            "a.x",
            vec![comment("a.x", (1, 1), "// @generated by x")],
            true,
        );
        let later = fragment(
            "a.x",
            vec![comment("a.x", (8, 8), "// @generated by x")],
            true,
        );
        let none = fragment(
            "a.x",
            vec![comment("a.x", (8, 8), "// @generated by x")],
            false,
        );

        assert!(marked(&found, head));
        assert!(!marked(&found, later), "after the first symbol");
        assert!(marked(&found, none), "a file with no symbol is all head");
    }

    #[test]
    fn a_marker_is_searched_in_every_language_and_in_comments_only() {
        let found = generated(MARKER);
        let in_a_string = fragment("a.x", Vec::new(), true);

        assert!(marked(
            &found,
            fragment("a.py", vec![comment("a.py", (1, 1), "# @generated")], true)
        ));
        assert!(marked(
            &found,
            fragment(
                "a.ts",
                vec![comment("a.ts", (1, 3), "/** @generated */")],
                true
            )
        ));
        assert!(
            !marked(&found, in_a_string),
            "text in a string is no comment"
        );
        assert!(!marked(
            &found,
            fragment("a.x", vec![comment("b.x", (1, 1), "// @generated")], true)
        ));
    }

    #[test]
    fn no_marker_marks_nothing_and_a_project_marker_is_its_own() {
        let none = generated("");
        let head = || {
            fragment(
                "a.x",
                vec![comment("a.x", (1, 1), "// Made by acme-gen")],
                true,
            )
        };

        assert!(!marked(&none, head()));
        assert!(marked(
            &generated("[generated]\nmarkers = ['Made by acme-gen']\n"),
            head()
        ));
    }

    #[test]
    fn files_name_generated_paths_with_the_syntax_of_ignores() {
        let found = generated("[generated]\nfiles = ['*.pb.go', 'gen/']\n");

        assert!(found.names(Path::new("api/x.pb.go")));
        assert!(found.names(Path::new("gen/a/b.go")));
        assert!(!found.names(Path::new("api/x.go")));
    }
}
