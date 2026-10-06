//! Which test names an owner test: the convention that ties a top-level test
//! function to one public symbol.

use lighthouse_model::{Project, Symbol, SymbolId, SymbolKind, TestCase};
use serde::Deserialize;

#[derive(Deserialize, Clone)]
pub(crate) struct Naming {
    /// Prefix of an owner test's name (`Test` in Go); a test without it is
    /// not an owner test.
    pub test_prefix: String,
    /// Names are compared in snake case (Rust tests are `snake_case`).
    pub snake_case: bool,
    /// `TestGet_Missing` is another owner of `Get`, not a case of it.
    pub variant_tests: bool,
}

/// How a test name maps to a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Match {
    Exact,
    Variant,
}

impl Naming {
    /// The tests of the modules that test `symbol`'s module from inside or
    /// outside, whose names map to `symbol`. Names resolve within the one
    /// module the symbol belongs to, so a name never maps to two symbols of
    /// different modules.
    pub(crate) fn owner_tests<'p>(
        &self,
        project: &'p Project,
        symbol: &Symbol,
    ) -> Vec<(&'p TestCase, Match)> {
        let module = symbol.id.module();
        let mut found = Vec::new();
        for tests in testing_modules(project, module) {
            for test in project.tests_in(&tests) {
                if let Some(kind) = self.maps(project, test, symbol) {
                    found.push((test, kind));
                }
            }
        }
        found
    }

    fn maps(&self, project: &Project, test: &TestCase, symbol: &Symbol) -> Option<Match> {
        let name = test_name(&test.symbol)?;
        let rest = name.strip_prefix(self.test_prefix.as_str())?;
        let key = self.key(project, symbol)?;
        if rest == key {
            return Some(Match::Exact);
        }
        let tail = rest.strip_prefix(key.as_str())?.strip_prefix('_')?;
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

/// Whether some test module tests `module` or an ancestor of it, with tests.
pub(crate) fn has_tests(project: &Project, module: &str) -> bool {
    project.modules.iter().any(|m| {
        let tested = m.test_of.as_deref().unwrap_or(&m.path);
        let covers = tested == module
            || module
                .strip_prefix(tested)
                .is_some_and(|rest| rest.starts_with('/'));
        covers && !project.tests_in(&m.path).is_empty()
    })
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

fn test_name(id: &SymbolId) -> Option<&str> {
    let (head, _) = id.as_str().rsplit_once('#')?;
    head.rsplit("::").next()
}
