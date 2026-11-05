//! The provider of files that only expose their syntax tree: the plugin gives
//! the tree of each file it owns, with the map from its values to the bytes of
//! the file, and the host selects the source regions that other providers
//! analyze. A plugin supplies the parser; this module is the rest of the
//! provider: reading the files (or their overlays) and the answers to
//! `initialize` and `index`.

use std::{collections::BTreeMap, fs, path::Path};

use serde_json::Value;

use super::provider::Provider;
use crate::{
    Comment, Conventions, EmbeddedFile, ErrorObject, FileInfo, FileRef, Fragment, Incomplete,
    IndexParams, IndexResult, LanguageManifest, Mapping, Source, UnitRef, UnitResult, Units, hash,
};

/// The capability of a provider that exposes syntax trees.
const SOURCE_AST: &str = "source-ast";

/// The capability of a provider that reads the overlays of a request instead of
/// the files on disk.
const OVERLAYS: &str = "overlays";

/// The priority of such a provider among those that match a file.
const PRIORITY: i32 = 20;

/// The tree of a file as JSON, and where each of its values lies in the file,
/// by JSON Pointer.
#[derive(Debug, Clone, PartialEq)]
pub struct HostTree {
    pub ast: Value,
    pub mapping: BTreeMap<String, Mapping>,
    /// Comments the parser can locate in the host document.
    pub comments: Vec<Comment>,
}

/// Why a file has no tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// The text is not a document of the language.
    Syntax(String),
    /// The document has no tree that the host can select from.
    Tree(String),
}

/// What a plugin knows of its language: which it is, and how a document becomes
/// a tree.
pub trait HostParser {
    fn language(&self) -> HostLanguage;
    fn parse(&self, text: &str) -> Result<HostTree, HostError>;
}

/// The [`Provider`] of a language that a parser reads.
pub struct HostProvider<P> {
    language: HostLanguage,
    parser: P,
}

impl<P: HostParser> HostProvider<P> {
    /// The provider of the language `parser` reads.
    pub fn new(parser: P) -> Self {
        Self {
            language: parser.language(),
            parser,
        }
    }

    /// The tree of the file at `path` with `text`, or what is missing of it.
    fn tree(&self, path: &str, text: &str) -> Result<(Source, Vec<Comment>), Incomplete> {
        let name = self.language.title;
        let gap = |reason: String| Incomplete {
            path: Some(path.to_owned()),
            reason,
        };
        match self.parser.parse(text) {
            Ok(tree) => Ok((
                Source {
                    path: path.to_owned(),
                    ast: tree.ast,
                    mapping: tree.mapping,
                },
                tree.comments,
            )),
            Err(HostError::Syntax(error)) => Err(gap(format!("invalid {name}: {error}"))),
            Err(HostError::Tree(reason)) => {
                Err(gap(format!("cannot expose {name} syntax tree: {reason}")))
            }
        }
    }
}

impl<P: HostParser> HostProvider<P> {
    /// Reads the regions that other files embed as documents of the language.
    /// The provider has no code model to give them, so a region is read as far
    /// as there is anything to say: one that is not a document is a notice,
    /// since what a file writes in a fence is not always the language's.
    fn region(&self, file: &EmbeddedFile, out: &mut UnitResult) -> Fragment {
        for source in &file.regions {
            match self.parser.parse(&source.text) {
                Ok(_) => {}
                Err(HostError::Syntax(error)) => out.notices.push(format!(
                    "{}: the {} at {} is not a valid {} document: {error}",
                    file.path, self.language.id, source.node_pointer, self.language.title
                )),
                Err(HostError::Tree(reason)) => out.incomplete.push(Incomplete {
                    path: Some(file.path.clone()),
                    reason: format!(
                        "cannot expose the {} at {}: {reason}",
                        self.language.title, source.node_pointer
                    ),
                }),
            }
        }
        Fragment {
            file: FileInfo {
                path: file.path.clone(),
                ..FileInfo::default()
            },
            ..Fragment::default()
        }
    }

    /// The files of a unit: a fragment each, and the tree of those that asked.
    fn files(
        &self,
        unit: &UnitRef,
        root: &Path,
        overlays: &BTreeMap<&str, &str>,
        out: &mut UnitResult,
    ) {
        for file in &unit.files {
            let text = match overlays.get(file.path.as_str()) {
                Some(text) => Ok((*text).to_owned()),
                None => fs::read_to_string(root.join(&file.path)),
            };
            let text = match text {
                Ok(text) => text,
                Err(error) => {
                    if file.source_ast {
                        out.incomplete.push(Incomplete {
                            path: Some(file.path.clone()),
                            reason: format!(
                                "cannot read {} host document: {error}",
                                self.language.title
                            ),
                        });
                    }
                    out.fragments.push(fragment_of(file, Vec::new()));
                    continue;
                }
            };
            match self.tree(&file.path, &text) {
                Ok((source, comments)) => {
                    if file.source_ast {
                        out.sources.push(source);
                    }
                    out.fragments.push(fragment_of(file, comments));
                }
                Err(gap) => {
                    if file.source_ast {
                        out.incomplete.push(gap);
                    }
                    out.fragments.push(fragment_of(file, Vec::new()));
                }
            }
        }
    }
}

impl<P: HostParser> Provider for HostProvider<P> {
    fn name(&self) -> &str {
        self.language.name
    }

    fn version(&self) -> &str {
        self.language.version
    }

    fn manifest(&self) -> Vec<LanguageManifest> {
        let language = self.language;
        vec![LanguageManifest {
            id: language.id.to_owned(),
            aliases: language
                .aliases
                .iter()
                .map(|alias| (*alias).to_owned())
                .collect(),
            globs: language
                .globs
                .iter()
                .map(|glob| (*glob).to_owned())
                .collect(),
            ignores: Vec::new(),
            priority: PRIORITY,
            fallback: false,
            conventions: Conventions::default(),
            capabilities: vec![SOURCE_AST.to_owned(), OVERLAYS.to_owned()],
            ordered_declarations: false,
            units: Units::default(),
        }]
    }

    fn index(&mut self, params: IndexParams) -> Result<IndexResult, ErrorObject> {
        let mut result = IndexResult::default();
        let overlays: BTreeMap<&str, &str> = params
            .context
            .overlays
            .iter()
            .flatten()
            .map(|overlay| (overlay.path.as_str(), overlay.text.as_str()))
            .collect();
        let root = Path::new(&params.project.root);
        for unit in &params.units {
            let mut out = UnitResult::default();
            self.files(unit, root, &overlays, &mut out);
            for file in &unit.regions {
                let fragment = self.region(file, &mut out);
                out.fragments.push(fragment);
            }
            // No importer reads a unit of a language that has no code model:
            // what its files hold is all its `api` says.
            let hashes: Vec<&str> = unit
                .files
                .iter()
                .map(|file| file.hash.as_str())
                .chain(unit.regions.iter().map(|file| file.hash.as_str()))
                .collect();
            out.body = hash::sha256(hashes.join("\n"));
            out.api = out.body.clone();
            out.id = unit.id.clone();
            result.units.push(out);
        }
        Ok(result)
    }
}

/// The language of a host provider, and the plugin that serves it.
#[derive(Debug, Clone, Copy)]
pub struct HostLanguage {
    /// The plugin id, which is the directory the plugin is installed in.
    pub name: &'static str,
    pub version: &'static str,
    /// The language id.
    pub id: &'static str,
    /// The name of the language in messages.
    pub title: &'static str,
    /// The names the language goes by in a document, see
    /// `LanguageManifest::aliases`.
    pub aliases: &'static [&'static str],
    pub globs: &'static [&'static str],
}

/// The fragment of a host file: the file alone, since the provider has no code
/// model, so that file rules see it whether or not its tree is wanted.
fn fragment_of(file: &FileRef, comments: Vec<Comment>) -> Fragment {
    Fragment {
        file: FileInfo {
            path: file.path.clone(),
            ..FileInfo::default()
        },
        comments,
        ..Fragment::default()
    }
}
