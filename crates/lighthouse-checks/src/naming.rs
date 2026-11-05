//! Which test names an owner test: the convention that ties a top-level test
//! function to one public symbol.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use lighthouse_model::ucm::{Model, Module, Symbol, SymbolId, SymbolKind, TestCase};
use lighthouse_plugin::TestNaming;
use serde::{Deserialize, Serialize};

use crate::{indexes::Indexes, layout::contains};

/// How a test name maps to a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Match {
    Exact,
    Variant,
}

impl Match {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Variant => "variant",
        }
    }
}

/// The tests of a project by the convention of a [`TestNaming`]. The tests of a
/// set of modules are indexed by their names once, however many symbols and
/// checks ask.
pub(crate) struct Tests<'p> {
    naming: TestNaming,
    project: &'p dyn Model,
    credit: Credit<'p>,
}

/// Where the index of the tests of some modules comes from: a cache of the
/// tests' own, or the run's indexes shared by every check.
enum Credit<'p> {
    Alone(Mutex<HashMap<Modules, Arc<Credited>>>),
    Shared(&'p dyn Indexes),
}

/// A prefix of test names and the modules whose tests are indexed.
type Modules = (String, Vec<String>);

/// The tests of some modules with the part of their names after the prefix,
/// sorted by it, and where each came in the modules' order.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credited {
    names: Vec<(String, usize, SymbolId)>,
}

impl<'p> Tests<'p> {
    pub(crate) fn new(
        naming: TestNaming,
        project: &'p dyn Model,
        indexes: &'p dyn Indexes,
    ) -> Self {
        Self {
            naming,
            project,
            credit: Credit::Shared(indexes),
        }
    }

    /// Tests whose index is made here, for what is computed once over a whole
    /// project.
    pub(crate) fn alone(naming: TestNaming, project: &'p dyn Model) -> Self {
        Self {
            naming,
            project,
            credit: Credit::Alone(Mutex::default()),
        }
    }

    /// The tests of the modules that test `symbol`'s module from inside or
    /// outside, whose names map to `symbol`. Names resolve within the one
    /// module the symbol belongs to, so a name never maps to two symbols of
    /// different modules when `ancestors` is off. With `ancestors`, the same
    /// test in an ancestor module may be credited to symbols in descendant modules, so the integration tests
    /// of a crate root count for every module of the crate, including the
    /// private ones whose public items the root re-exports.
    pub(crate) fn credited_tests(&self, symbol: &Symbol) -> Vec<(&'p TestCase, Match)> {
        let module = symbol.id.module();
        if !self.naming.ancestors {
            return self.mapped(symbol, testing_modules(self.project, module));
        }
        let modules = self
            .project
            .modules()
            .filter(|m| m.path == module || tests_ancestor(m, module))
            .map(|m| m.path.clone())
            .collect();
        self.mapped(symbol, modules)
    }

    fn mapped(&self, symbol: &Symbol, modules: Vec<String>) -> Vec<(&'p TestCase, Match)> {
        let naming = &self.naming;
        let Some(key) = key(naming, self.project, symbol) else {
            return Vec::new();
        };
        let index = self.index(modules);
        let from = index
            .names
            .partition_point(|(rest, ..)| rest.as_str() < key.as_str());
        let mut found: Vec<(usize, &'p TestCase, Match)> = index.names[from..]
            .iter()
            .take_while(|(rest, ..)| rest.starts_with(key.as_str()))
            .filter_map(|(rest, order, id)| {
                let kind = maps(naming, self.project, rest, &key, symbol)?;
                Some((*order, self.project.test(id)?, kind))
            })
            .collect();
        found.sort_by_key(|(order, ..)| *order);
        found
            .into_iter()
            .map(|(_, test, kind)| (test, kind))
            .collect()
    }

    fn index(&self, modules: Vec<String>) -> Arc<Credited> {
        let prefix = self.naming.prefix.as_str();
        match &self.credit {
            Credit::Shared(indexes) => indexes.credited(prefix, &modules),
            Credit::Alone(cache) => {
                let key = (prefix.to_owned(), modules);
                let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
                let found = cache.entry(key).or_insert_with_key(|(prefix, modules)| {
                    Arc::new(credited(self.project, prefix, modules))
                });
                Arc::clone(found)
            }
        }
    }
}

/// The index of the tests of `modules` by the part of their names after
/// `prefix`.
pub fn credited(model: &dyn Model, prefix: &str, modules: &[String]) -> Credited {
    let mut names = Vec::new();
    for module in modules {
        for test in model.tests_in(module) {
            let rest = test_name(&test.symbol).and_then(|name| name.strip_prefix(prefix));
            if let Some(rest) = rest {
                names.push((rest.to_owned(), names.len(), test.symbol.clone()));
            }
        }
    }
    names.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    Credited { names }
}

pub(crate) fn snake(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let boundary = i > 0
            && c.is_uppercase()
            && (chars[i - 1].is_lowercase()
                || chars.get(i + 1).is_some_and(|n| n.is_lowercase())
                    && chars[i - 1].is_uppercase());
        if boundary {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// How the test whose name is `rest` after the prefix maps to `symbol`, whose
/// name for tests is `key`.
fn maps(
    naming: &TestNaming,
    project: &dyn Model,
    rest: &str,
    key: &str,
    symbol: &Symbol,
) -> Option<Match> {
    if rest == key {
        return Some(Match::Exact);
    }
    let tail = rest.strip_prefix(key)?.strip_prefix('_')?;
    if tail.is_empty() || !naming.variants || names_a_member(project, symbol, tail) {
        return None;
    }
    Some(Match::Variant)
}

/// The name tests use for a symbol: the name, or `Owner_name` for a method.
fn key(naming: &TestNaming, project: &dyn Model, symbol: &Symbol) -> Option<String> {
    let spell = |s: &str| {
        if naming.snake_case {
            snake(s)
        } else {
            s.to_owned()
        }
    };
    match (&symbol.owner, symbol.kind) {
        (None, SymbolKind::Function | SymbolKind::Type | SymbolKind::Interface) => {
            Some(spell(&symbol.name))
        }
        (Some(owner), SymbolKind::Method) => {
            let owner = project.symbol(owner)?;
            Some(format!("{}_{}", spell(&owner.name), spell(&symbol.name)))
        }
        _ => None,
    }
}

/// Whether `tail` starts with the name of a method of `symbol`, so that
/// `TestStore_Get_Missing` belongs to `Get`, not to `Store`.
fn names_a_member(project: &dyn Model, symbol: &Symbol, tail: &str) -> bool {
    let first = tail.split('_').next().unwrap_or(tail);
    project.members(&symbol.id).iter().any(|id| {
        project.symbol(id).is_some_and(|m| {
            m.kind == SymbolKind::Method && (m.name == first || snake(&m.name) == first)
        })
    })
}

/// Modules whose tests count for `module`: itself, and those that test it.
fn testing_modules(project: &dyn Model, module: &str) -> Vec<String> {
    project
        .modules()
        .filter(|m| m.path == module || m.test_of.as_deref() == Some(module))
        .map(|m| m.path.clone())
        .collect()
}

/// Whether `tests` tests `module` or a module that contains it (a nested
/// module path, `crate/a/b` inside `crate`).
fn tests_ancestor(tests: &Module, module: &str) -> bool {
    tests
        .test_of
        .as_deref()
        .is_some_and(|tested| contains(tested, module))
}

fn test_name(id: &SymbolId) -> Option<&str> {
    let (head, _) = id.as_str().rsplit_once('#')?;
    head.rsplit("::").next()
}
