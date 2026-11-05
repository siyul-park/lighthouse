//! What the keys of cached results are built from, read off the graph: the hash
//! of a file's `resolved` slice, of its `incoming` and of the shape of the
//! files it is tied to. Nothing is hashed again; a hash is the graph's own,
//! validated without loading a value.
//!
//! The `shape` of a file is what a neighbor sees of the symbols it declares
//! (identity, kind, visibility, owner, flags, the number of callers, callees,
//! references and members) but not where in the file they lie, so that an edit
//! that only moves a neighbor changes nothing; `at` is the same with the place
//! of each symbol, for the rules that read it.

use std::{collections::HashMap, path::Path};

use lighthouse_graph::{Error, Graph, Options, Query};
use lighthouse_model::ucm::{
    Fragment, Incoming, Node, Project, Resolved, Symbol, SymbolId, Table, Target,
};
use lighthouse_spec::key;
use serde::{Deserialize, Serialize};

use super::model::{EDGES, Host, INCOMING, PROJECT, RESOLVED, SYMBOLS};
use crate::cache::{Digest, KeyBuilder};

pub(crate) static SHAPE: Query<String, Shape> = Query::new("shape", shape);
pub(crate) static TIES: Query<(String, bool), Digest> = Query::new("ties", ties);
/// The keys of a project: one per file and one for all of it.
#[derive(Debug, Default)]
pub struct Keys {
    project: Digest,
    files: HashMap<String, FileKey>,
    /// By language: the paths of the project's files with the slice of those of
    /// that language, for the rules whose findings in a file depend on them.
    siblings: HashMap<String, Digest>,
}

/// What the keys of one file's results are built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileKey {
    pub slice: Digest,
    pub neighbors: Digest,
    pub neighbors_at: Digest,
}

impl Keys {
    /// The keys of `project`, which the graph holds.
    pub(crate) fn new(graph: &Graph, project: &Project, positions: bool) -> Result<Self, Error> {
        let mut files: Vec<(String, String)> = project
            .files
            .iter()
            .map(|file| (key(&file.path), file.lang.clone()))
            .collect();
        files.sort();
        let paths: Vec<String> = files.iter().map(|(path, _)| path.clone()).collect();
        let found = graph.par_map(paths, move |graph, path| {
            Ok::<_, Error>(FileKey {
                slice: RESOLVED.hash(graph, path)?,
                neighbors: TIES.hash(graph, &(path.clone(), false))?,
                neighbors_at: if positions {
                    TIES.hash(graph, &(path.clone(), true))?
                } else {
                    [0; 32]
                },
            })
        });
        let mut whole = KeyBuilder::new();
        let mut paths = KeyBuilder::new();
        let mut languages: HashMap<&str, KeyBuilder> = HashMap::new();
        let mut keys = HashMap::new();
        for ((path, language), found) in files.iter().zip(found) {
            let found = found?;
            whole.part(path).part(found.slice);
            paths.part(path);
            languages
                .entry(language)
                .or_default()
                .part(path)
                .part(found.slice);
            keys.insert(path.clone(), found);
        }
        whole
            .part(SYMBOLS.hash(graph, &())?)
            .part(EDGES.hash(graph, &())?);
        let paths = paths.finish();
        Ok(Self {
            project: whole.finish(),
            siblings: languages
                .into_iter()
                .map(|(language, text)| {
                    let mut key = KeyBuilder::new();
                    key.part(paths).part(text.finish());
                    (language.to_owned(), key.finish())
                })
                .collect(),
            files: keys,
        })
    }

    /// The keys of the project the given fragments make, one unit per file,
    /// without a run: what a cached result of a file would be keyed by.
    pub fn of(fragments: &[Fragment]) -> Result<Self, crate::Error> {
        let mut graph = Graph::in_memory(Options::default())?;
        Host::set_units(&mut graph, fragments)?;
        let project = graph.query(&PROJECT, &())?;
        Ok(Self::new(&graph, &project, true)?)
    }

    /// The digest of the paths of the project's files and the slice of those
    /// of `language`; `None` for a language the project has no file of.
    pub fn siblings(&self, language: &str) -> Option<&Digest> {
        self.siblings.get(language)
    }

    /// The digest of everything the project holds.
    pub fn project(&self) -> &Digest {
        &self.project
    }

    /// The keys of a file; `None` for a path the project does not know.
    pub fn file(&self, path: &Path) -> Option<&FileKey> {
        self.files.get(&key(path))
    }
}

/// The ties of the symbols of one file, with the shape of the other ends.
struct Tied<'a> {
    graph: &'a Graph,
    table: &'a Table,
    all: Vec<Shape>,
}

impl Tied<'_> {
    fn edge(
        &mut self,
        direction: &[u8],
        at: &SymbolId,
        kind: [u8; 2],
        other: End<'_>,
    ) -> Result<(), Error> {
        let shape = self.end(other.symbol, &other.written)?;
        self.tie(direction, at, kind, shape);
        Ok(())
    }

    /// The shape of the file of `symbol`, or the end as written when it is
    /// not a symbol of the project.
    fn end(&self, symbol: Option<&SymbolId>, written: &[u8]) -> Result<Shape, Error> {
        match symbol.and_then(|id| self.table.entry(id)) {
            Some(entry) => Ok(*self.graph.query(&SHAPE, &key(&entry.file))?),
            None => {
                let mut key = KeyBuilder::new();
                key.part(written);
                let digest = key.finish();
                Ok(Shape {
                    plain: digest,
                    at: digest,
                })
            }
        }
    }

    /// One tie of a symbol: the direction, the symbol, the kind and the
    /// resolution of the edge, and what is at the other end.
    fn tie(&mut self, direction: &[u8], at: &SymbolId, kind: [u8; 2], other: Shape) {
        let make = |digest: Digest| {
            let mut key = KeyBuilder::new();
            key.part(direction)
                .part(at.as_str())
                .part(kind)
                .part(digest);
            key.finish()
        };
        self.all.push(Shape {
            plain: make(other.plain),
            at: make(other.at),
        });
    }
}

/// The far end of an edge: the symbol it names, if it names one, and the end
/// as written.
struct End<'a> {
    symbol: Option<&'a SymbolId>,
    written: Vec<u8>,
}

impl<'a> End<'a> {
    fn of_node(node: &'a Node) -> Self {
        Self {
            symbol: match node {
                Node::Symbol(id) => Some(id),
                Node::Module(_) => None,
            },
            written: postcard::to_stdvec(node).unwrap_or_default(),
        }
    }

    fn of_target(target: &'a Target) -> Self {
        Self {
            symbol: match target {
                Target::Resolved(Node::Symbol(id)) => Some(id),
                _ => None,
            },
            written: postcard::to_stdvec(target).unwrap_or_default(),
        }
    }
}

/// A digest without and with the places of the symbols it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Shape {
    pub plain: Digest,
    pub at: Digest,
}

/// The shape of the symbols a file declares.
fn shape(graph: &Graph, path: &String) -> Result<Shape, Error> {
    let resolved = graph.query(&RESOLVED, path)?;
    let table = graph.query(&SYMBOLS, &())?;
    let incoming = graph.query(&INCOMING, path)?;
    let mut described: Vec<Shape> = declared(&resolved, &table)
        .map(|symbol| describe(&resolved, &table, &incoming, symbol))
        .collect();
    described.sort_unstable_by_key(|d| d.at);
    let mut plain = KeyBuilder::new();
    let mut at = KeyBuilder::new();
    for d in &described {
        plain.part(d.plain);
        at.part(d.at);
    }
    Ok(Shape {
        plain: plain.finish(),
        at: at.finish(),
    })
}

/// What the symbols of a file are tied to: each edge that starts or ends at
/// one, the owner of one and its members, each with the shape of the file of
/// the symbol at the other end.
fn ties(graph: &Graph, (path, at): &(String, bool)) -> Result<Digest, Error> {
    let resolved = graph.query(&RESOLVED, path)?;
    let table = graph.query(&SYMBOLS, &())?;
    let incoming = graph.query(&INCOMING, path)?;
    let mut tied = Tied {
        graph,
        table: &table,
        all: Vec::new(),
    };
    for symbol in declared(&resolved, &table) {
        let id = &symbol.id;
        if let Some(owner) = &symbol.owner {
            let shape = tied.end(Some(owner), b"")?;
            tied.tie(b"owner", id, [0, 0], shape);
        }
        let outgoing = resolved
            .slice
            .fragment
            .edges
            .iter()
            .filter(|e| matches!(&e.from, Node::Symbol(from) if from == id));
        for edge in outgoing {
            let kind = [edge.kind as u8, edge.resolution as u8];
            tied.edge(b"out", id, kind, End::of_target(&edge.to))?;
        }
        let Some(tie) = incoming.by_symbol.get(id) else {
            continue;
        };
        for member in &tie.members {
            let shape = tied.end(Some(member), b"")?;
            tied.tie(b"member", id, [0, 0], shape);
        }
        for (kind, resolution, from) in &tie.edges {
            let kind = [*kind as u8, *resolution as u8];
            tied.edge(b"in", id, kind, End::of_node(from))?;
        }
    }
    let side = |tie: &Shape| if *at { tie.at } else { tie.plain };
    let mut all = tied.all;
    all.sort_unstable_by_key(side);
    let mut key = KeyBuilder::new();
    for tie in &all {
        key.part(side(tie));
    }
    Ok(key.finish())
}

/// The symbols of the slice that the table says are declared in this file.
fn declared<'a>(
    resolved: &'a Resolved,
    table: &'a Table,
) -> impl Iterator<Item = &'a Symbol> + use<'a> {
    let path = &resolved.slice.path;
    resolved
        .slice
        .fragment
        .symbols
        .iter()
        .filter(move |symbol| table.entry(&symbol.id).is_some_and(|e| &e.file == path))
}

/// A description of a symbol: every field the facts of a node carry about a
/// neighbor except its place, which only `at` includes. The fields are fed
/// with their lengths, so that none runs into the next.
fn describe(resolved: &Resolved, table: &Table, incoming: &Incoming, symbol: &Symbol) -> Shape {
    let id = &symbol.id;
    let part = &resolved.slice.fragment;
    let owner = symbol.owner.as_ref().and_then(|o| table.entry(o));
    let file = part.files.iter().find(|f| f.path == symbol.file);
    let tie = incoming.by_symbol.get(id).cloned().unwrap_or_default();
    let in_test = file.is_some_and(|f| f.test)
        || table
            .module(id.module())
            .is_some_and(|m| m.test_of.is_some());
    let flags = [
        symbol.doc.is_some(),
        in_test,
        file.is_some_and(|f| f.test),
        file.is_some_and(|f| f.generated),
        part.functions
            .iter()
            .any(|f| f.symbol == *id && f.implementation),
    ];
    let counts = [
        tie.callers.len(),
        tie.callees.len(),
        tie.references.len(),
        tie.members.len(),
    ];
    let mut key = KeyBuilder::new();
    key.part(id.as_str())
        .part(&symbol.name)
        .part(symbol.kind.as_str())
        .part([
            symbol.visibility as u8,
            symbol.role.map_or(255, |r| r as u8),
        ])
        .part(symbol.owner.as_ref().map_or("", |o| o.as_str()))
        .part(owner.map_or("", |o| o.kind.as_str()))
        .part(
            owner
                .map_or(Path::new(""), |o| o.file.as_path())
                .as_os_str()
                .as_encoded_bytes(),
        )
        .part(symbol.file.as_os_str().as_encoded_bytes())
        .part(file.map_or("", |f| f.lang.as_str()))
        .part(flags.map(u8::from));
    for n in counts {
        key.part((n as u64).to_le_bytes());
    }
    let plain = key.clone().finish();
    let span = symbol.span;
    for n in [span.start.line, span.start.col, span.end.line, span.end.col] {
        key.part(n.to_le_bytes());
    }
    Shape {
        plain,
        at: key.finish(),
    }
}
