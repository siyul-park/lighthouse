//! The digests of one file's surroundings: what its symbols are tied to, and
//! the description of each symbol that a neighbor sees.

use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
};

use lighthouse_model::{Edge, Node, Project, Symbol, SymbolId, Target, hash::Hasher};
use rayon::prelude::*;
use serde::Serialize;

use crate::{
    Error,
    digest::{Digest, FileDigest, digest_of, feed},
};

/// What the passes over files share: the project, the description of every
/// symbol, and the edges that end at each.
pub(crate) struct Ties<'p> {
    project: &'p Project,
    described: HashMap<&'p SymbolId, Described>,
    incoming: HashMap<&'p SymbolId, Vec<&'p Edge>>,
}

impl<'p> Ties<'p> {
    pub(crate) fn of(project: &'p Project) -> Self {
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
    pub(crate) fn file(&self, path: &Path) -> Result<FileDigest, Error> {
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
            // A module the project does not list is hashed by its path.
            match project.module(module) {
                Some(module) => feed(&mut slice, module)?,
                None => feed(&mut slice, module)?,
            }
        }
        // The edges and ties of a file come in the order the providers made
        // them in, which is not the same from run to run.
        let mut near = Hasher::new();
        let mut near_at = Hasher::new();
        parts.edges.sort_unstable();
        parts.edges.iter().for_each(|edge| slice.update(edge));
        parts.ties.sort_unstable();
        parts.ties.iter().for_each(|tie| near.update(tie.plain));
        parts.ties.iter().for_each(|tie| near_at.update(tie.at));
        Ok(FileDigest {
            slice: slice.finish_bytes(),
            neighbors: near.finish_bytes(),
            neighbors_at: near_at.finish_bytes(),
        })
    }

    /// The owner of a symbol, and the members it has.
    fn owned(&self, slice: &mut Hasher, parts: &mut Parts, symbol: &Symbol) -> Result<(), Error> {
        let project = self.project;
        if let Some(owner) = symbol.owner.as_ref().and_then(|o| project.symbol(o)) {
            feed(slice, &(owner.kind.as_str(), &owner.name))?;
            if let Some(described) = self.described.get(&owner.id) {
                parts
                    .ties
                    .push(tie(b"owner", &symbol.id, [0, 0], described.pair()));
            }
        }
        for member in project.members(&symbol.id) {
            if let Some(described) = self.described.get(member) {
                parts
                    .ties
                    .push(tie(b"member", &symbol.id, [0, 0], described.pair()));
            }
        }
        Ok(())
    }

    /// The edges that start and end at a symbol. The ones that start at it
    /// are part of the file's own slice.
    fn edges(&self, parts: &mut Parts, symbol: &Symbol) -> Result<(), Error> {
        for edge in self.project.edges_from(&symbol.id) {
            parts.edges.push(edge_digest(edge)?);
            let other = self.end(&edge.to)?;
            parts
                .ties
                .push(tie(b"out", &symbol.id, kind_of(edge), other));
        }
        for edge in self.incoming.get(&symbol.id).into_iter().flatten() {
            let other = self.end(&edge.from)?;
            parts
                .ties
                .push(tie(b"in", &symbol.id, kind_of(edge), other));
        }
        Ok(())
    }

    /// What identifies an end of an edge: the description of its symbol, or
    /// the end as written when it is not a symbol of the project.
    fn end<T: Serialize + SymbolOf>(&self, end: &T) -> Result<(Digest, Digest), Error> {
        match end.symbol().and_then(|id| self.described.get(id)) {
            Some(described) => Ok(described.pair()),
            None => {
                let written = digest_of(end)?;
                Ok((written, written))
            }
        }
    }

    /// What belongs to no file: summaries and tests of symbols that are not
    /// declared, and edges that do not start at a symbol of the project.
    pub(crate) fn orphans(&self, whole: &mut Hasher) -> Result<(), Error> {
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
    ties: Vec<Tie>,
}

/// A tie as the `neighbors` and the `neighbors_at` digest see it.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Tie {
    plain: Digest,
    at: Digest,
}

/// A symbol as its neighbors see it, without and with its place.
#[derive(Clone, Copy)]
struct Described {
    plain: Digest,
    at: Digest,
}

impl Described {
    fn pair(self) -> (Digest, Digest) {
        (self.plain, self.at)
    }
}

/// A description of a symbol: every field the facts of a node carry about a
/// neighbor except its place, which only `at` includes. The fields are fed
/// with their lengths, so that none runs into the next.
fn describe(project: &Project, symbol: &Symbol) -> Described {
    let id = &symbol.id;
    let owner = symbol.owner.as_ref().and_then(|o| project.symbol(o));
    let file = project.file(&symbol.file);
    let counts = [
        project.callers(id).len(),
        project.callees(id).len(),
        project.references(id).len(),
        project.members(id).len(),
    ];
    let flags = [
        symbol.doc.is_some(),
        project.in_test(id),
        file.is_some_and(|f| f.test),
        file.is_some_and(|f| f.generated),
        project.function(id).is_some_and(|f| f.implementation),
    ];
    let mut hasher = Hasher::new();
    let mut part = |bytes: &[u8]| {
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    part(id.as_str().as_bytes());
    part(symbol.name.as_bytes());
    part(symbol.kind.as_str().as_bytes());
    part(&[
        symbol.visibility as u8,
        symbol.role.map_or(255, |r| r as u8),
    ]);
    part(
        symbol
            .owner
            .as_ref()
            .map_or("", SymbolId::as_str)
            .as_bytes(),
    );
    part(owner.map_or("", |o| o.kind.as_str()).as_bytes());
    part(
        owner
            .map_or(Path::new(""), |o| o.file.as_path())
            .as_os_str()
            .as_encoded_bytes(),
    );
    part(symbol.file.as_os_str().as_encoded_bytes());
    part(file.map_or("", |f| f.lang.as_str()).as_bytes());
    part(&flags.map(u8::from));
    counts.iter().for_each(|n| part(&(*n as u64).to_le_bytes()));
    let plain = hasher.clone().finish_bytes();
    let span = symbol.span;
    for n in [span.start.line, span.start.col, span.end.line, span.end.col] {
        hasher.update(n.to_le_bytes());
    }
    Described {
        plain,
        at: hasher.finish_bytes(),
    }
}

/// One tie of a symbol: the direction, the symbol, the kind of the edge and
/// what is at the other end.
fn tie(direction: &[u8], at: &SymbolId, kind: [u8; 2], (plain, placed): (Digest, Digest)) -> Tie {
    let make = |other: Digest| {
        let mut hasher = Hasher::new();
        for part in [direction, at.as_str().as_bytes(), &kind, &other] {
            hasher.update((part.len() as u64).to_le_bytes());
            hasher.update(part);
        }
        hasher.finish_bytes()
    };
    Tie {
        plain: make(plain),
        at: make(placed),
    }
}

/// The kind and the resolution of an edge, as two numbers.
fn kind_of(edge: &Edge) -> [u8; 2] {
    [edge.kind as u8, edge.resolution as u8]
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
