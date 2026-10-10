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
        let mut near = Hasher::new();
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
            self.owned(&mut slice, &mut near, symbol)?;
            self.edges(&mut slice, &mut near, symbol)?;
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
        Ok(FileDigest {
            slice: slice.finish_bytes(),
            neighbors: near.finish_bytes(),
        })
    }

    /// The owner of a symbol, and the members it has.
    fn owned(&self, slice: &mut Hasher, near: &mut Hasher, symbol: &Symbol) -> Result<(), Error> {
        let project = self.project;
        if let Some(owner) = symbol.owner.as_ref().and_then(|o| project.symbol(o)) {
            feed(slice, &(owner.kind.as_str(), &owner.name))?;
            if let Some(described) = self.described.get(&owner.id) {
                tie(near, b"owner", &symbol.id, "", described);
            }
        }
        for member in project.members(&symbol.id) {
            if let Some(described) = self.described.get(member) {
                tie(near, b"member", &symbol.id, "", described);
            }
        }
        Ok(())
    }

    /// The edges that start and end at a symbol. The ones that start at it
    /// are part of the file's own slice.
    fn edges(&self, slice: &mut Hasher, near: &mut Hasher, symbol: &Symbol) -> Result<(), Error> {
        for edge in self.project.edges_from(&symbol.id) {
            feed(slice, edge)?;
            let kind = format!("{}|{:?}", edge.kind.as_str(), edge.resolution);
            tie(near, b"out", &symbol.id, &kind, &self.end(&edge.to)?);
        }
        for edge in self.incoming.get(&symbol.id).into_iter().flatten() {
            let kind = format!("{}|{:?}", edge.kind.as_str(), edge.resolution);
            tie(near, b"in", &symbol.id, &kind, &self.end(&edge.from)?);
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
        for edge in &project.edges {
            let declared = match &edge.from {
                Node::Symbol(id) => project.symbol(id).is_some(),
                Node::Module(_) => false,
            };
            if !declared {
                feed(whole, edge)?;
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

/// Writes one tie of a symbol into the neighbors hasher of its file.
fn tie(near: &mut Hasher, direction: &[u8], at: &SymbolId, kind: &str, other: &[u8]) {
    for part in [direction, at.as_str().as_bytes(), kind.as_bytes(), other] {
        near.update((part.len() as u64).to_le_bytes());
        near.update(part);
    }
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
