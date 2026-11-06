//! The symbol table: what resolving an edge target and linking the edges ask
//! of the project as a whole, and nothing else. It holds no span and no body,
//! so an edit that only changes what a declaration contains leaves it equal.

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound,
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use super::{File, Fragment, Module, Slice, SymbolId, SymbolKind};

/// The symbols of a project by id, the first declaration of an id kept, with
/// its modules as they are reconciled and the files that are example code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    pub(super) symbols: BTreeMap<SymbolId, Entry>,
    pub(super) modules: BTreeMap<String, Module>,
    /// The symbols of the project's own code by the name they are declared
    /// under, for the edges that name no more than that.
    pub(super) named: BTreeMap<String, Vec<SymbolId>>,
    pub(super) examples: BTreeSet<PathBuf>,
}

/// What the table knows of a declared symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub file: PathBuf,
    pub kind: SymbolKind,
    pub owner: Option<SymbolId>,
    pub name: String,
}

impl Table {
    /// The table of the outlines of a project's slices, in the order of their
    /// paths: the first declaration of an id wins, as it does when the
    /// fragments are merged.
    pub fn new<'a>(outlines: impl IntoIterator<Item = &'a Outline>) -> Self {
        let mut symbols: BTreeMap<SymbolId, Entry> = BTreeMap::new();
        let mut modules: BTreeMap<String, Module> = BTreeMap::new();
        let mut files: BTreeMap<&std::path::Path, File> = BTreeMap::new();
        for outline in outlines {
            for (id, entry) in &outline.symbols {
                symbols.entry(id.clone()).or_insert_with(|| entry.clone());
            }
            for module in &outline.modules {
                match modules.get_mut(&module.path) {
                    Some(kept) => reconcile(kept, module),
                    None => {
                        modules.insert(module.path.clone(), module.clone());
                    }
                }
            }
            for file in &outline.files {
                match files.get_mut(file.path.as_path()) {
                    Some(kept) => kept.facts.extend(file.facts.clone()),
                    None => {
                        files.insert(file.path.as_path(), file.clone());
                    }
                }
            }
        }
        let examples = files
            .values()
            .filter(|file| file.is_example())
            .map(|file| file.path.clone())
            .collect();
        Self::with_names(symbols, modules, examples)
    }

    /// The table of a merged fragment: its symbols are already one per id.
    pub fn of(all: &Fragment) -> Self {
        let symbols = all
            .symbols
            .iter()
            .map(|s| (s.id.clone(), Entry::of(s)))
            .collect();
        let modules = all
            .modules
            .iter()
            .map(|m| (m.path.clone(), m.clone()))
            .collect();
        let examples = all
            .files
            .iter()
            .filter(|file| file.is_example())
            .map(|file| file.path.clone())
            .collect();
        Self::with_names(symbols, modules, examples)
    }

    fn with_names(
        symbols: BTreeMap<SymbolId, Entry>,
        modules: BTreeMap<String, Module>,
        examples: BTreeSet<PathBuf>,
    ) -> Self {
        let mut named: BTreeMap<String, Vec<SymbolId>> = BTreeMap::new();
        for (id, entry) in &symbols {
            if !modules.get(id.module()).is_some_and(|m| m.example) {
                named
                    .entry(entry.name.clone())
                    .or_default()
                    .push(id.clone());
            }
        }
        Self {
            symbols,
            modules,
            named,
            examples,
        }
    }

    /// What the table knows of `id`.
    pub fn entry(&self, id: &SymbolId) -> Option<&Entry> {
        self.symbols.get(id)
    }

    /// The reconciled module at `path`.
    pub fn module(&self, path: &str) -> Option<&Module> {
        self.modules.get(path)
    }

    /// Every module, sorted by path.
    pub fn modules(&self) -> impl Iterator<Item = &Module> {
        self.modules.values()
    }

    /// Whether the file at `path` is a host whose regions are all example code.
    pub fn is_example(&self, path: &std::path::Path) -> bool {
        self.examples.contains(path)
    }

    /// The entries whose id starts with `head` and `#`: every kind a path
    /// without a kind may name, in id order.
    pub(super) fn bare(&self, head: &str) -> Vec<(&SymbolId, SymbolKind)> {
        if head.contains('#') {
            return Vec::new();
        }
        let from = format!("{head}#");
        self.symbols
            .range::<str, _>((Bound::Included(from.as_str()), Bound::Unbounded))
            .take_while(|(id, _)| id.as_str().starts_with(&from))
            .map(|(id, entry)| (id, entry.kind))
            .collect()
    }
}

/// What the table takes of a file's slice: its declarations as entries, its
/// modules, and the facts of its files. Not a span and not a content hash, so
/// that an edit that only moves things leaves it equal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outline {
    pub symbols: Vec<(SymbolId, Entry)>,
    pub modules: Vec<Module>,
    pub files: Vec<File>,
}

impl Outline {
    /// The outline of a slice.
    pub fn new(slice: &Slice) -> Self {
        let part = &slice.fragment;
        Self {
            symbols: part
                .symbols
                .iter()
                .map(|symbol| (symbol.id.clone(), Entry::of(symbol)))
                .collect(),
            modules: part.modules.clone(),
            files: part
                .files
                .iter()
                .map(|file| File {
                    path: file.path.clone(),
                    lang: String::new(),
                    hash: String::new(),
                    generated: false,
                    test: false,
                    facts: file.facts.clone(),
                })
                .collect(),
        }
    }
}

impl Entry {
    fn of(symbol: &super::Symbol) -> Self {
        Self {
            file: symbol.file.clone(),
            kind: symbol.kind,
            owner: symbol.owner.clone(),
            name: symbol.name.clone(),
        }
    }
}

/// A module that a region declares and a file of the project does too is the
/// project's: example code and a language the host states are not its.
pub(super) fn reconcile(kept: &mut Module, later: &Module) {
    kept.example &= later.example;
    if kept.language != later.language {
        kept.language = None;
    }
}
