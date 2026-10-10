//! Which test names an owner test: the convention that ties a top-level test
//! function to one public symbol.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use lighthouse_model::{Module, Project, Symbol, SymbolId, SymbolKind, TestCase};
use serde::Deserialize;

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Naming {
    /// Prefix of an owner test's name (`Test` in Go); a test without it is
    /// not an owner test.
    pub test_prefix: String,
    /// Names are compared in snake case (Rust tests are `snake_case`).
    pub snake_case: bool,
    /// `TestGet_Missing` is another owner of `Get`, not a case of it.
    pub variant_tests: bool,
    /// The tests of a module that tests an ancestor module also name the
    /// symbols of the nested module (the integration tests of a crate test
    /// all of it). Only owner-test reads it.
    #[serde(default)]
    pub ancestor_tests: bool,
}

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

/// The tests of a project by the convention of a [`Naming`]. The tests of a
/// set of modules are indexed by their names once per run, however many
/// symbols and checks ask.
pub(crate) struct Tests<'p> {
    naming: Naming,
    project: &'p Project,
    indexes: Arc<Indexes>,
}

/// The indexes of a run, by test prefix and the modules indexed; they live in
/// the run's [`lighthouse_plugin::Memo`], shared by every check.
#[derive(Default)]
pub(crate) struct Indexes(Mutex<HashMap<IndexKey, Arc<Index>>>);

type IndexKey = (String, Vec<String>);

/// The tests of some modules with the part of their names after the prefix,
/// sorted by it, and where each came in the modules' order.
struct Index {
    names: Vec<(String, usize, SymbolId)>,
}

impl<'p> Tests<'p> {
    pub(crate) fn new(naming: Naming, project: &'p Project, indexes: Arc<Indexes>) -> Self {
        Self {
            naming,
            project,
            indexes,
        }
    }

    /// The tests of the modules that test `symbol`'s module from inside or
    /// outside, whose names map to `symbol`. Names resolve within the one
    /// module the symbol belongs to, so a name never maps to two symbols of
    /// different modules. With `ancestorTests` also the tests of a module
    /// that tests an ancestor of the symbol's module, so the integration tests
    /// of a crate root count for every module of the crate, including the
    /// private ones whose public items the root re-exports.
    pub(crate) fn credited_tests(&self, symbol: &Symbol) -> Vec<(&'p TestCase, Match)> {
        let module = symbol.id.module();
        if !self.naming.ancestor_tests {
            return self.mapped(symbol, testing_modules(self.project, module));
        }
        let modules = self
            .project
            .modules
            .iter()
            .filter(|m| m.path == module || tests_ancestor(m, module))
            .map(|m| m.path.clone())
            .collect();
        self.mapped(symbol, modules)
    }

    fn mapped(&self, symbol: &Symbol, modules: Vec<String>) -> Vec<(&'p TestCase, Match)> {
        let naming = &self.naming;
        let Some(key) = naming.key(self.project, symbol) else {
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
                let kind = naming.maps(self.project, rest, &key, symbol)?;
                Some((*order, self.project.test(id)?, kind))
            })
            .collect();
        found.sort_by_key(|(order, ..)| *order);
        found
            .into_iter()
            .map(|(_, test, kind)| (test, kind))
            .collect()
    }

    fn index(&self, modules: Vec<String>) -> Arc<Index> {
        let key = (self.naming.test_prefix.clone(), modules);
        let mut indexes = self
            .indexes
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(index) = indexes.get(&key) {
            return Arc::clone(index);
        }
        let mut names = Vec::new();
        for module in &key.1 {
            for test in self.project.tests_in(module) {
                let rest = test_name(&test.symbol)
                    .and_then(|name| name.strip_prefix(self.naming.test_prefix.as_str()));
                if let Some(rest) = rest {
                    names.push((rest.to_owned(), names.len(), test.symbol.clone()));
                }
            }
        }
        names.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
        let index = Arc::new(Index { names });
        indexes.insert(key, Arc::clone(&index));
        index
    }
}

impl Naming {
    /// The naming convention a decision's options declare (`testPrefix`,
    /// `snakeCase`, `variantTests`, `ancestorTests`); `None` for a decision
    /// that declares none.
    pub(crate) fn from_options(
        options: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<Self> {
        options.get("testPrefix")?;
        serde_json::from_value(serde_json::Value::Object(options.clone())).ok()
    }

    /// How the test whose name is `rest` after the prefix maps to `symbol`,
    /// whose name for tests is `key`.
    fn maps(&self, project: &Project, rest: &str, key: &str, symbol: &Symbol) -> Option<Match> {
        if rest == key {
            return Some(Match::Exact);
        }
        let tail = rest.strip_prefix(key)?.strip_prefix('_')?;
        if tail.is_empty() || !self.variant_tests || self.names_a_member(project, symbol, tail) {
            return None;
        }
        Some(Match::Variant)
    }

    /// The name tests use for a symbol: the name, or `Owner_name` for a method.
    fn key(&self, project: &Project, symbol: &Symbol) -> Option<String> {
        let spell = |s: &str| {
            if self.snake_case {
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
    fn names_a_member(&self, project: &Project, symbol: &Symbol, tail: &str) -> bool {
        let first = tail.split('_').next().unwrap_or(tail);
        project.members(&symbol.id).iter().any(|id| {
            project.symbol(id).is_some_and(|m| {
                m.kind == SymbolKind::Method && (m.name == first || snake(&m.name) == first)
            })
        })
    }
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

/// Modules whose tests count for `module`: itself, and those that test it.
fn testing_modules(project: &Project, module: &str) -> Vec<String> {
    project
        .modules
        .iter()
        .filter(|m| m.path == module || m.test_of.as_deref() == Some(module))
        .map(|m| m.path.clone())
        .collect()
}

/// Whether `tests` tests `module` or a module that contains it (a nested
/// module path, `crate/a/b` inside `crate`).
fn tests_ancestor(tests: &Module, module: &str) -> bool {
    tests.test_of.as_deref().is_some_and(|tested| {
        tested == module
            || module
                .strip_prefix(tested)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn test_name(id: &SymbolId) -> Option<&str> {
    let (head, _) = id.as_str().rsplit_once('#')?;
    head.rsplit("::").next()
}
