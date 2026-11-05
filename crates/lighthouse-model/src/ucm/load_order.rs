//! What the declarations of a file need of each other as the file loads, for a
//! language that evaluates them in the order they are written (Python,
//! JavaScript, TypeScript). A use is `when: load` when the provider says it is
//! evaluated as the module executes; calling a function at load runs its body,
//! so the uses of every function a load-time use calls, and of those it calls
//! in turn, are load-time uses of the code that calls it. A need is stated
//! between siblings, the two declarations of the file (or of one class body)
//! that a move can put in the wrong order.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use super::{
    Edge, EdgeKind, File, Fragment, Index, Node, Project, Resolution, Symbol, SymbolId, Target,
    When,
};
use crate::{Position, Span};

/// A name that must be bound above the code that uses it as the file loads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LoadNeed {
    /// The code that needs the name.
    pub user: LoadPoint,
    /// The declaration that binds it: a sibling of `user`, or of the
    /// declaration that holds the code of `user`.
    pub binding: SymbolId,
}

/// The code that runs as a file loads and uses a name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LoadPoint {
    /// A declaration that runs its own code as the file reaches it: a variable
    /// with its initializer, a class with its bases and body, a decorated
    /// function, a test suite. It is placed where its extent starts.
    Declaration(SymbolId),
    /// Code of the file that no declaration holds (its top level), placed
    /// where the use is.
    Site(Span),
}

impl LoadNeed {
    /// The need without the place of top-level code: the declaration that
    /// uses, or `None` for code that is not one, with the binding.
    pub fn key(&self) -> (Option<&SymbolId>, &SymbolId) {
        match &self.user {
            LoadPoint::Declaration(id) => (Some(id), &self.binding),
            LoadPoint::Site(_) => (None, &self.binding),
        }
    }
}

/// Whose code a load use is: the chain of declarations that hold it, outermost
/// first, or the top level of the file at the place of the use.
enum Holder<'a> {
    Symbol(Vec<&'a Symbol>),
    Module,
}

impl Project {
    /// The needs of `file` as it loads, in a fixed order; none for a file of a
    /// language that does not order its declarations. Top-level code that
    /// needs a binding more than once counts at the use the project keeps (the
    /// first its provider listed, which is the earliest).
    pub fn load_needs(&self, file: &Path) -> Vec<LoadNeed> {
        if !self.file(file).is_some_and(File::is_ordered) {
            return Vec::new();
        }
        let mut needs: BTreeSet<LoadNeed> = BTreeSet::new();
        for at in self.inner.index.loads.get(file).into_iter().flatten() {
            self.needs_of(&self.inner.all.edges[*at], file, &mut needs);
        }
        let mut earliest: BTreeMap<&SymbolId, Span> = BTreeMap::new();
        for need in &needs {
            if let LoadPoint::Site(span) = need.user {
                let seen = earliest.entry(&need.binding).or_insert(span);
                *seen = (*seen).min(span);
            }
        }
        needs
            .iter()
            .filter(|n| match n.user {
                LoadPoint::Site(span) => earliest[&n.binding] == span,
                LoadPoint::Declaration(_) => true,
            })
            .cloned()
            .collect()
    }

    /// The needs of `file` that its text breaks: a binding that starts at or
    /// below the code that needs it.
    pub fn load_breaks(&self, file: &Path) -> Vec<LoadNeed> {
        self.load_needs(file)
            .into_iter()
            .filter(
                |need| match (self.load_place(&need.user), self.symbol(&need.binding)) {
                    (Some(user), Some(binding)) => start_of(binding) >= user,
                    _ => false,
                },
            )
            .collect()
    }

    /// Where the code of `point` starts.
    pub fn load_place(&self, point: &LoadPoint) -> Option<Position> {
        match point {
            LoadPoint::Declaration(id) => self.symbol(id).map(start_of),
            LoadPoint::Site(span) => Some(span.start),
        }
    }

    /// The needs a load edge states, with those of the functions it calls.
    fn needs_of(&self, origin: &Edge, file: &Path, needs: &mut BTreeSet<LoadNeed>) {
        let Some(holder) = self.holder_of(origin, file) else {
            return;
        };
        let mut seen = BTreeSet::new();
        let mut pending = vec![origin];
        while let Some(edge) = pending.pop() {
            let Some(target) = self.target_in(edge, file) else {
                continue;
            };
            if let Some(need) = self.need(&holder, target, origin.site) {
                needs.insert(need);
            }
            if edge.kind == EdgeKind::Calls && seen.insert(&target.id) {
                // Calling a function runs it: what its body uses, the caller
                // needs as it loads.
                pending.extend(self.edges_from(&target.id));
            }
        }
    }

    /// The symbol a use names, when it is in `file` and an edge that counts.
    fn target_in(&self, edge: &Edge, file: &Path) -> Option<&Symbol> {
        let counts = matches!(
            edge.kind,
            EdgeKind::Calls | EdgeKind::References | EdgeKind::Implements
        ) && edge.resolution != Resolution::Heuristic;
        let Target::Resolved(Node::Symbol(to)) = &edge.to else {
            return None;
        };
        self.symbol(to).filter(|s| counts && s.file == file)
    }

    /// Whose code a load edge is: the declarations that hold it, or the top
    /// level of the file when it has a place there.
    fn holder_of(&self, edge: &Edge, file: &Path) -> Option<Holder<'_>> {
        match &edge.from {
            Node::Symbol(id) => {
                let symbol = self.symbol(id).filter(|s| s.file == file)?;
                Some(Holder::Symbol(self.chain(symbol)))
            }
            Node::Module(_) => edge.site.map(|_| Holder::Module),
        }
    }

    /// The need of `holder` for `target`, between the siblings that the two
    /// chains of owners lead to; none when one holds the other.
    fn need(&self, holder: &Holder, target: &Symbol, site: Option<Span>) -> Option<LoadNeed> {
        let down = self.chain(target);
        let (user, binding) = match holder {
            Holder::Module => (LoadPoint::Site(site?), *down.first()?),
            Holder::Symbol(up) => {
                let shared = up
                    .iter()
                    .zip(&down)
                    .take_while(|(a, b)| a.id == b.id)
                    .count();
                let (user, binding) = (up.get(shared)?, *down.get(shared)?);
                let user = if user.extent.is_some() {
                    LoadPoint::Declaration(user.id.clone())
                } else {
                    LoadPoint::Site(site.unwrap_or(user.span))
                };
                (user, binding)
            }
        };
        Some(LoadNeed {
            user,
            binding: binding.id.clone(),
        })
    }

    /// The owners of `symbol` that are in its file, outermost first, then the
    /// symbol itself.
    fn chain<'a>(&'a self, symbol: &'a Symbol) -> Vec<&'a Symbol> {
        let mut chain = vec![symbol];
        let mut at = symbol;
        while let Some(owner) = at
            .owner
            .as_ref()
            .and_then(|id| self.symbol(id))
            .filter(|o| o.file == symbol.file && !chain.iter().any(|c| c.id == o.id))
        {
            chain.push(owner);
            at = owner;
        }
        chain.reverse();
        chain
    }
}

/// The positions in `edges` of the load edges of each file.
pub(super) fn index(all: &Fragment, index: &Index) -> BTreeMap<PathBuf, Vec<usize>> {
    let mut loads: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    for (at, edge) in all.edges.iter().enumerate() {
        if edge.when != When::Load {
            continue;
        }
        let file = match &edge.from {
            Node::Symbol(id) => index
                .symbols
                .get(id)
                .map(|&s| all.symbols[s].file.as_path()),
            Node::Module(_) => index.origins.get(&at).map(PathBuf::as_path),
        };
        if let Some(file) = file {
            loads.entry(file.to_owned()).or_default().push(at);
        }
    }
    loads
}

fn start_of(symbol: &Symbol) -> Position {
    symbol.extent.unwrap_or(symbol.span).start
}
