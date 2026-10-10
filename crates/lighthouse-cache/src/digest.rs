//! The hashes of a merged project that keys are built from, computed once per
//! run from what the merge already holds, the files side by side.
//!
//! Per file there are two. The `slice` covers what the file itself contributes:
//! its record, symbols, function summaries, tests, comments, outgoing edges,
//! the modules and owners of its symbols. The `neighbors` digest covers what
//! its symbols are tied to: each edge that starts or ends at one, the owner of
//! one and its members, each with a description of the symbol at the other end
//! (identity, kind, visibility, owner, file, test and generation flags, the
//! number of its callers, callees, references and members) but not where in
//! its file it lies, so that an edit that only moves a neighbor changes
//! nothing. The whole-project digest is made of every slice and what belongs
//! to no file.

use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
};

use lighthouse_model::{Document, Edge, Node, Project, Symbol, SymbolId, Target, hash::Hasher};
use rayon::prelude::*;
use serde::Serialize;

use crate::Error;

/// A SHA-256.
pub type Digest = [u8; 32];

/// What the keys of one file's results are built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileDigest {
    pub slice: Digest,
    pub neighbors: Digest,
}

/// The digests of a project: one per file and one for all of it.
#[derive(Debug, Default)]
pub struct Digests {
    project: Digest,
    files: HashMap<PathBuf, FileDigest>,
}

impl Digests {
    /// Hashes `project`.
    pub fn of(project: &Project) -> Result<Self, Error> {
        let ties = Ties::of(project);
        let mut paths: BTreeSet<&Path> = project.files.iter().map(|f| f.path.as_path()).collect();
        paths.extend(
            project
                .symbols
                .iter()
                .map(|s| s.file.as_path())
                .chain(project.comments.iter().map(|c| c.file.as_path())),
        );
        let paths: Vec<&Path> = paths.into_iter().collect();
        let digests = paths
            .par_iter()
            .map(|path| ties.file(path))
            .collect::<Result<Vec<FileDigest>, Error>>()?;
        let mut whole = Hasher::new();
        for (path, digest) in paths.iter().zip(&digests) {
            whole.update(path.as_os_str().as_encoded_bytes());
            whole.update(digest.slice);
        }
        for module in &project.modules {
            feed(&mut whole, module)?;
        }
        ties.orphans(&mut whole)?;
        Ok(Self {
            project: whole.finish_bytes(),
            files: paths.into_iter().map(Path::to_owned).zip(digests).collect(),
        })
    }

    /// The digest of everything the project holds.
    pub fn project(&self) -> &Digest {
        &self.project
    }

    /// The digests of a file; `None` for a path the project does not know.
    pub fn file(&self, path: &Path) -> Option<&FileDigest> {
        self.files.get(path)
    }
}

/// What the passes over files share: the project, the description of every
/// symbol, and the edges that end at each.
struct Ties<'p> {
    project: &'p Project,
    described: HashMap<&'p SymbolId, Digest>,
    incoming: HashMap<&'p SymbolId, Vec<&'p Edge>>,
}

impl<'p> Ties<'p> {
    fn of(project: &'p Project) -> Self {
        let described = project
            .symbols
            .par_iter()
            .map(|symbol| (&symbol.id, describe(project, symbol)))
            .collect();
        let mut incoming: HashMap<&SymbolId, Vec<&Edge>> = HashMap::new();
        for edge in &project.edges {
            if let Target::Resolved(Node::Symbol(to)) = &edge.to {
                incoming.entry(to).or_default().push(edge);
            }
        }
        Self {
            project,
            described,
            incoming,
        }
    }

    /// The digests of one file.
    fn file(&self, path: &Path) -> Result<FileDigest, Error> {
        let project = self.project;
        let mut slice = Hasher::new();
        let mut parts = Parts::default();
        let mut modules = BTreeSet::new();
        if let Some(file) = project.file(path) {
            feed(&mut slice, file)?;
        }
        for symbol in project.symbols_in(path) {
            feed(&mut slice, symbol)?;
            if let Some(function) = project.function(&symbol.id) {
                feed(&mut slice, function)?;
            }
            if let Some(case) = project.test(&symbol.id) {
                feed(&mut slice, case)?;
            }
            modules.insert(symbol.id.module());
            self.owned(&mut slice, &mut parts, symbol)?;
            self.edges(&mut parts, symbol)?;
        }
        for comment in project.comments_in(path) {
            feed(&mut slice, comment)?;
        }
        for module in modules {
            match project.module(module) {
                Some(module) => feed(&mut slice, module)?,
                None => feed(&mut slice, module)?,
            }
        }
        // The edges and ties of a file come in the order the providers made
        // them in, which is not the same from run to run.
        let mut near = Hasher::new();
        parts.edges.sort_unstable();
        parts.edges.iter().for_each(|edge| slice.update(edge));
        parts.ties.sort_unstable();
        parts.ties.iter().for_each(|tie| near.update(tie));
        Ok(FileDigest {
            slice: slice.finish_bytes(),
            neighbors: near.finish_bytes(),
        })
    }

    /// The owner of a symbol, and the members it has.
    fn owned(&self, slice: &mut Hasher, parts: &mut Parts, symbol: &Symbol) -> Result<(), Error> {
        let project = self.project;
        if let Some(owner) = symbol.owner.as_ref().and_then(|o| project.symbol(o)) {
            feed(slice, &(owner.kind.as_str(), &owner.name))?;
            if let Some(described) = self.described.get(&owner.id) {
                parts.ties.push(tie(b"owner", &symbol.id, "", described));
            }
        }
        for member in project.members(&symbol.id) {
            if let Some(described) = self.described.get(member) {
                parts.ties.push(tie(b"member", &symbol.id, "", described));
            }
        }
        Ok(())
    }

    /// The edges that start and end at a symbol. The ones that start at it
    /// are part of the file's own slice.
    fn edges(&self, parts: &mut Parts, symbol: &Symbol) -> Result<(), Error> {
        for edge in self.project.edges_from(&symbol.id) {
            parts.edges.push(edge_digest(edge)?);
            let kind = format!("{}|{:?}", edge.kind.as_str(), edge.resolution);
            let other = self.end(&edge.to)?;
            parts.ties.push(tie(b"out", &symbol.id, &kind, &other));
        }
        for edge in self.incoming.get(&symbol.id).into_iter().flatten() {
            let kind = format!("{}|{:?}", edge.kind.as_str(), edge.resolution);
            let other = self.end(&edge.from)?;
            parts.ties.push(tie(b"in", &symbol.id, &kind, &other));
        }
        Ok(())
    }

    /// What identifies an end of an edge: the description of its symbol, or
    /// the end as written when it is not a symbol of the project.
    fn end<T: Serialize + SymbolOf>(&self, end: &T) -> Result<Vec<u8>, Error> {
        match end.symbol().and_then(|id| self.described.get(id)) {
            Some(described) => Ok(described.to_vec()),
            None => Ok(serde_json::to_vec(end)?),
        }
    }

    /// What belongs to no file: summaries and tests of symbols that are not
    /// declared, and edges that do not start at a symbol of the project.
    fn orphans(&self, whole: &mut Hasher) -> Result<(), Error> {
        let project = self.project;
        for function in &project.functions {
            if project.symbol(&function.symbol).is_none() {
                feed(whole, function)?;
            }
        }
        for case in &project.tests {
            if project.symbol(&case.symbol).is_none() {
                feed(whole, case)?;
            }
        }
        let mut edges = Vec::new();
        for edge in &project.edges {
            let declared = match &edge.from {
                Node::Symbol(id) => project.symbol(id).is_some(),
                Node::Module(_) => false,
            };
            if !declared {
                edges.push(edge_digest(edge)?);
            }
        }
        edges.sort_unstable();
        edges.iter().for_each(|edge| whole.update(edge));
        self.sites(whole)
    }

    /// Where each symbol is referred to. Only the rules that read the whole
    /// project can see these, so they are not part of any file's digests.
    fn sites(&self, whole: &mut Hasher) -> Result<(), Error> {
        for symbol in &self.project.symbols {
            for site in self.project.sites(&symbol.id) {
                feed(
                    whole,
                    &(
                        &symbol.id,
                        &site.file,
                        &site.from,
                        site.kind,
                        site.resolution,
                        site.span,
                    ),
                )?;
            }
        }
        Ok(())
    }
}

/// The symbol an end of an edge names, if it names one.
trait SymbolOf {
    fn symbol(&self) -> Option<&SymbolId>;
}

impl SymbolOf for Node {
    fn symbol(&self) -> Option<&SymbolId> {
        match self {
            Self::Symbol(id) => Some(id),
            Self::Module(_) => None,
        }
    }
}

impl SymbolOf for Target {
    fn symbol(&self) -> Option<&SymbolId> {
        match self {
            Self::Resolved(node) => node.symbol(),
            Self::Path(_) => None,
        }
    }
}

/// What a file's edges and ties come to before they are put in order.
#[derive(Default)]
struct Parts {
    edges: Vec<Digest>,
    ties: Vec<Digest>,
}

/// The digest of the documents of a project, which no file digest covers.
pub fn documents(documents: &[Document]) -> Result<Digest, Error> {
    let mut hasher = Hasher::new();
    for document in documents {
        feed(
            &mut hasher,
            &(
                &document.file,
                document.at,
                &document.kind,
                &document.name,
                &document.uid,
                &document.labels,
            ),
        )?;
    }
    Ok(hasher.finish_bytes())
}

/// A description of a symbol that does not say where in its file it is.
fn describe(project: &Project, symbol: &Symbol) -> Digest {
    let id = &symbol.id;
    let owner = symbol.owner.as_ref().and_then(|o| project.symbol(o));
    let file = project.file(&symbol.file);
    let text = format!(
        "{}|{}|{}|{:?}|{}|{}|{}|{:?}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        id.as_str(),
        symbol.name,
        symbol.kind.as_str(),
        symbol.visibility,
        symbol.owner.as_ref().map_or("", SymbolId::as_str),
        owner.map_or("", |o| o.kind.as_str()),
        owner.map_or_else(String::new, |o| o.file.to_string_lossy().into_owned()),
        symbol.role,
        symbol.doc.is_some(),
        project.in_test(id),
        symbol.file.display(),
        file.map_or("", |f| f.lang.as_str()),
        file.is_some_and(|f| f.test),
        file.is_some_and(|f| f.generated),
        project.function(id).is_some_and(|f| f.implementation),
        project.callers(id).len(),
        project.callees(id).len(),
        project.references(id).len(),
        project.members(id).len(),
    );
    let mut hasher = Hasher::new();
    hasher.update(text);
    hasher.finish_bytes()
}

/// One tie of a symbol: the direction, the symbol, the kind of the edge and
/// what is at the other end.
fn tie(direction: &[u8], at: &SymbolId, kind: &str, other: &[u8]) -> Digest {
    let mut hasher = Hasher::new();
    for part in [direction, at.as_str().as_bytes(), kind.as_bytes(), other] {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hasher.finish_bytes()
}

/// The hash of an edge without its site: which of the sites of a relation is
/// kept depends on the order the providers answered in, and the sites are
/// hashed on their own. Whether there is one stays.
fn edge_digest(edge: &Edge) -> Result<Digest, Error> {
    digest_of(&(
        edge.kind,
        &edge.from,
        &edge.to,
        edge.resolution,
        edge.site.is_some(),
    ))
}

/// The hash of the JSON of `value`.
fn digest_of<T: Serialize + ?Sized>(value: &T) -> Result<Digest, Error> {
    let mut hasher = Hasher::new();
    feed(&mut hasher, value)?;
    Ok(hasher.finish_bytes())
}

/// Feeds the JSON of `value` to `hasher`.
fn feed<T: Serialize + ?Sized>(hasher: &mut Hasher, value: &T) -> Result<(), Error> {
    thread_local! {
        static JSON: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }
    JSON.with_borrow_mut(|json| {
        json.clear();
        serde_json::to_writer(&mut *json, value)?;
        hasher.update(&*json);
        hasher.update([0]);
        Ok(())
    })
}
