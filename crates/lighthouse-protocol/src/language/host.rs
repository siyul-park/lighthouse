//! The provider of files that only expose their syntax tree: the plugin gives
//! the tree of each file it owns, with the map from its values to the bytes of
//! the file, and the host selects the source regions that other providers
//! analyze. A plugin supplies the parser; this module is the rest of the
//! provider: reading the files (or their overlays), the answers to
//! `initialize` and `index`, and the loop.

use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, Write},
    path::Path,
};

use serde_json::Value;

use super::serve::Handler;
use crate::{
    Conventions, ErrorObject, FileInfo, Fragment, Incomplete, IndexInput, IndexParams, IndexResult,
    InitializeParams, InitializeResult, Mapping, ProviderManifest, Source, serve,
};

/// The capability of a provider that exposes syntax trees.
const SOURCE_AST: &str = "source-ast";

/// The priority of such a provider among those that match a file.
const PRIORITY: i32 = 20;

/// The tree of a file as JSON, and where each of its values lies in the file,
/// by JSON Pointer.
#[derive(Debug, Clone, PartialEq)]
pub struct HostTree {
    pub ast: Value,
    pub mapping: BTreeMap<String, Mapping>,
}

/// Why a file has no tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// The text is not a document of the language.
    Syntax(String),
    /// The document has no tree that the host can select from.
    Tree(String),
}

/// The language of a host provider, and the plugin that serves it.
#[derive(Debug, Clone, Copy)]
pub struct HostLanguage {
    /// The plugin id, which is the directory the plugin is installed in.
    pub plugin: &'static str,
    pub version: &'static str,
    /// The language id.
    pub id: &'static str,
    /// The name of the language in messages.
    pub name: &'static str,
    pub globs: &'static [&'static str],
}

/// What a plugin knows of its language: how a document becomes a tree.
pub trait HostParser {
    fn parse(&self, text: &str) -> Result<HostTree, HostError>;
}

struct HostProvider<P> {
    language: HostLanguage,
    parser: P,
}

impl<P: HostParser> HostProvider<P> {
    /// The tree of the file at `path` with `text`, or what is missing of it.
    fn tree(&self, path: &str, text: &str) -> Result<Source, Incomplete> {
        let name = self.language.name;
        let gap = |reason: String| Incomplete {
            path: Some(path.to_owned()),
            reason,
        };
        match self.parser.parse(text) {
            Ok(tree) => Ok(Source {
                path: path.to_owned(),
                ast: tree.ast,
                mapping: tree.mapping,
            }),
            Err(HostError::Syntax(error)) => Err(gap(format!("invalid {name}: {error}"))),
            Err(HostError::Tree(reason)) => {
                Err(gap(format!("cannot expose {name} syntax tree: {reason}")))
            }
        }
    }
}

impl<P: HostParser> Handler for HostProvider<P> {
    fn initialize(&mut self, _: InitializeParams) -> Result<InitializeResult, ErrorObject> {
        let language = self.language;
        Ok(InitializeResult::new(
            language.plugin,
            language.version,
            vec![ProviderManifest {
                id: language.id.to_owned(),
                globs: language
                    .globs
                    .iter()
                    .map(|glob| (*glob).to_owned())
                    .collect(),
                priority: PRIORITY,
                fallback: false,
                conventions: Conventions::default(),
                capabilities: vec![SOURCE_AST.to_owned()],
            }],
            Vec::new(),
        ))
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
        for input in &params.inputs {
            let file = match input {
                IndexInput::File {
                    file,
                    source_ast: true,
                } => file,
                IndexInput::File { .. } => continue,
                IndexInput::Regions { regions } => {
                    result.incomplete.push(Incomplete {
                        path: Some(regions.path.clone()),
                        reason: format!(
                            "{} provider does not analyze selected source regions",
                            self.language.id
                        ),
                    });
                    continue;
                }
            };
            let text = match overlays.get(file.path.as_str()) {
                Some(text) => Ok((*text).to_owned()),
                None => fs::read_to_string(Path::new(&params.project.root).join(&file.path)),
            };
            let text = match text {
                Ok(text) => text,
                Err(error) => {
                    result.incomplete.push(Incomplete {
                        path: Some(file.path.clone()),
                        reason: format!(
                            "cannot read {} host document: {error}",
                            self.language.name
                        ),
                    });
                    continue;
                }
            };
            match self.tree(&file.path, &text) {
                Ok(source) => {
                    result.sources.push(source);
                    result.fragments.push(Fragment {
                        file: FileInfo {
                            path: file.path.clone(),
                            ..FileInfo::default()
                        },
                        ..Fragment::default()
                    });
                }
                Err(gap) => result.incomplete.push(gap),
            }
        }
        Ok(result)
    }
}

/// The provider of `language` that `parser` reads.
pub fn host_handler(language: HostLanguage, parser: impl HostParser) -> impl Handler {
    HostProvider { language, parser }
}

/// Serves `language` from `reader` to `writer` until the host exits, as a
/// plugin serves its standard input and output.
pub fn serve_host(
    language: HostLanguage,
    parser: impl HostParser,
    reader: impl BufRead,
    writer: impl Write,
) {
    let mut provider = host_handler(language, parser);
    if let Err(error) = serve(reader, writer, &mut provider) {
        eprintln!("{}: {error}", language.plugin);
    }
}
