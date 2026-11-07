//! The indexes over the whole project that facts read: who takes which type,
//! the tests an owner is credited with, the split of a method, the cycle of a
//! module and the like. Each is an aggregate of the whole project with a
//! projection per key, so that a fact reads the one entry it needs; the
//! reference is [`Computed`], which makes each from a model on first use.

use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use lighthouse_model::ucm::{Model, Symbol, SymbolId};
use lighthouse_plugin::{Error, Queries, Workspace};
use serde::{Deserialize, Serialize};

use crate::{
    builder::{
        cohesion::types,
        cycle::cycles,
        effective::{Takers, takers},
        homonyms::names,
        imports::imports,
        owned::owned,
        parent::{Used, Uses, parents},
        split::splits,
        tested,
    },
    fact::{Fact, subject_key, subject_of},
    layout::contains,
    metrics::{
        MEASURES, Measures, measure,
        propagation::{self, Stability, System},
    },
    naming::{Credited, credited},
};

/// The kinds of the indexes, which [`Queries`] answers by name.
pub const TAKERS: &str = "takers";
pub const USERS: &str = "users";
pub const OWNED: &str = "owned";
pub const NAMES: &str = "names";
pub const IMPORTS: &str = "imports";
pub const CYCLES: &str = "cycles";
pub const TYPES: &str = "types";
pub const PARENTS: &str = "parents";
pub const SPLIT: &str = "split";
pub const REACH: &str = "reach";
pub const STABILITY: &str = "stability";
pub const TESTED: &str = "tested";
pub const CREDITED: &str = "credited";

/// What a node of a graph reaches, with how many nodes the graph has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reached {
    pub nodes: usize,
    pub stability: Stability,
}

impl Reached {
    /// What `system` says of the node `name`; `None` when it is no node.
    pub fn of(system: &System, name: &str) -> Option<Self> {
        Some(Self {
            nodes: system.nodes.len(),
            stability: system.of(name)?.clone(),
        })
    }
}

/// What facts read of the whole project, one entry at a time.
pub trait Indexes {
    /// The functions that take the type, by its kind-less id.
    fn takers(&self, ty: &str) -> Vec<SymbolId>;
    /// The symbols that call or reference `id`.
    fn users(&self, id: &SymbolId) -> Vec<SymbolId>;
    /// The symbols a test is credited to.
    fn owned(&self, test: &SymbolId) -> Vec<SymbolId>;
    /// The exported types of the project with this name.
    fn names(&self, name: &str) -> Vec<SymbolId>;
    /// The import paths of a module.
    fn imports(&self, module: &str) -> Vec<String>;
    /// The modules on a cycle of imports with `module`.
    fn cycle(&self, module: &str) -> Vec<String>;
    /// The types of a module.
    fn types(&self, module: &str) -> Vec<Fact>;
    /// What the production symbols use of a module.
    fn parent(&self, module: &str) -> Used;
    /// The split of a method.
    fn split(&self, id: &SymbolId) -> Option<Fact>;
    /// What a symbol reaches in the graph of symbols.
    fn reach(&self, id: &SymbolId) -> Option<Reached>;
    /// What a module reaches in the graph of modules.
    fn stability(&self, module: &str) -> Option<Reached>;
    /// Whether some test module tests `module` or one that holds it.
    fn tested(&self, module: &str) -> bool;
    /// The tests of `modules` by the part of their names after `prefix`.
    fn credited(&self, prefix: &str, modules: &[String]) -> Arc<Credited>;
    /// What is measured of a function, method or variable; `None` for a
    /// symbol the metrics analyzer does not measure.
    fn measured(&self, symbol: &Symbol) -> Option<Measures>;
    /// What went wrong in answering, for a source that can fail.
    fn failure(&self) -> Option<Error> {
        None
    }
}

/// The indexes made from a model, each when first asked for.
pub struct Computed<'a> {
    model: &'a (dyn Model + Sync),
    ws: &'a Workspace,
    takers: OnceLock<Takers>,
    owned: OnceLock<BTreeMap<SymbolId, Vec<SymbolId>>>,
    names: OnceLock<BTreeMap<String, Vec<SymbolId>>>,
    imports: OnceLock<BTreeMap<String, Vec<String>>>,
    cycles: OnceLock<BTreeMap<String, Vec<String>>>,
    types: OnceLock<BTreeMap<String, Vec<Fact>>>,
    parents: OnceLock<Uses>,
    splits: OnceLock<BTreeMap<SymbolId, Fact>>,
    modules: OnceLock<System>,
    symbols: OnceLock<System>,
    tested: OnceLock<BTreeSet<String>>,
    credited: Mutex<BTreeMap<String, Arc<Credited>>>,
}

impl<'a> Computed<'a> {
    pub fn new(model: &'a (dyn Model + Sync), ws: &'a Workspace) -> Self {
        Self {
            model,
            ws,
            takers: OnceLock::new(),
            owned: OnceLock::new(),
            names: OnceLock::new(),
            imports: OnceLock::new(),
            cycles: OnceLock::new(),
            types: OnceLock::new(),
            parents: OnceLock::new(),
            splits: OnceLock::new(),
            modules: OnceLock::new(),
            symbols: OnceLock::new(),
            tested: OnceLock::new(),
            credited: Mutex::default(),
        }
    }
}

impl Indexes for Computed<'_> {
    fn measured(&self, symbol: &Symbol) -> Option<Measures> {
        measure(self.model, symbol)
    }

    fn takers(&self, ty: &str) -> Vec<SymbolId> {
        let index = self.takers.get_or_init(|| takers(self.model));
        index.takers.get(ty).cloned().unwrap_or_default()
    }

    fn users(&self, id: &SymbolId) -> Vec<SymbolId> {
        let index = self.takers.get_or_init(|| takers(self.model));
        index.users.get(id).cloned().unwrap_or_default()
    }

    fn owned(&self, test: &SymbolId) -> Vec<SymbolId> {
        let index = self.owned.get_or_init(|| owned(self.model, self.ws));
        index.get(test).cloned().unwrap_or_default()
    }

    fn names(&self, name: &str) -> Vec<SymbolId> {
        let index = self.names.get_or_init(|| names(self.model));
        index.get(name).cloned().unwrap_or_default()
    }

    fn imports(&self, module: &str) -> Vec<String> {
        let index = self.imports.get_or_init(|| imports(self.model));
        index.get(module).cloned().unwrap_or_default()
    }

    fn cycle(&self, module: &str) -> Vec<String> {
        let index = self.cycles.get_or_init(|| cycles(self.model));
        index.get(module).cloned().unwrap_or_default()
    }

    fn types(&self, module: &str) -> Vec<Fact> {
        let index = self.types.get_or_init(|| types(self.model));
        index.get(module).cloned().unwrap_or_default()
    }

    fn parent(&self, module: &str) -> Used {
        self.parents.get_or_init(|| parents(self.model)).of(module)
    }

    fn split(&self, id: &SymbolId) -> Option<Fact> {
        let index = self.splits.get_or_init(|| splits(self.model));
        index.get(id).cloned()
    }

    fn reach(&self, id: &SymbolId) -> Option<Reached> {
        let system = self
            .symbols
            .get_or_init(|| propagation::symbols(self.model));
        Reached::of(system, id.as_str())
    }

    fn stability(&self, module: &str) -> Option<Reached> {
        let system = self
            .modules
            .get_or_init(|| propagation::modules(self.model));
        Reached::of(system, module)
    }

    fn tested(&self, module: &str) -> bool {
        let roots = self.tested.get_or_init(|| tested(self.model));
        roots.iter().any(|root| contains(root, module))
    }

    fn credited(&self, prefix: &str, modules: &[String]) -> Arc<Credited> {
        let key = credit_key(prefix, modules);
        let mut cache = self.credited.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(
            cache
                .entry(key)
                .or_insert_with(|| Arc::new(credited(self.model, prefix, modules))),
        )
    }
}

/// The indexes as queries, for what pulls them by name over a model alone.
impl Queries for Computed<'_> {
    fn pull(&self, kind: &str, key: &str) -> Result<Arc<dyn Any + Send + Sync>, Error> {
        if let Some(answer) = self.of_symbol(kind, key) {
            return Ok(answer);
        }
        if let Some(answer) = self.of_name(kind, key) {
            return Ok(answer);
        }
        match kind {
            CREDITED => {
                let (prefix, modules) = credit_of(key);
                Ok(Arc::new(self.credited(prefix, &modules).as_ref().clone()))
            }
            MEASURES => {
                let (_, file) = subject_of(key);
                let measured: BTreeMap<SymbolId, Measures> = self
                    .model
                    .symbols_in(std::path::Path::new(file))
                    .filter_map(|symbol| Some((symbol.id.clone(), self.measured(symbol)?)))
                    .collect();
                Ok(Arc::new(measured))
            }
            other => Err(Error::Failed(format!("`{other}` is no query of a model"))),
        }
    }
}

impl Computed<'_> {
    /// The indexes whose key is a symbol.
    fn of_symbol(&self, kind: &str, key: &str) -> Option<Arc<dyn Any + Send + Sync>> {
        let id = SymbolId::parse(key).unwrap_or_else(|| {
            SymbolId::new(key, &[], "", lighthouse_model::ucm::SymbolKind::Function)
        });
        Some(match kind {
            USERS => Arc::new(self.users(&id)),
            OWNED => Arc::new(self.owned(&id)),
            SPLIT => Arc::new(self.split(&id)),
            REACH => Arc::new(self.reach(&id)),
            _ => return None,
        })
    }

    /// The indexes whose key is a name.
    fn of_name(&self, kind: &str, key: &str) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(match kind {
            TAKERS => Arc::new(self.takers(key)),
            NAMES => Arc::new(self.names(key)),
            IMPORTS => Arc::new(self.imports(key)),
            CYCLES => Arc::new(self.cycle(key)),
            TYPES => Arc::new(self.types(key)),
            PARENTS => Arc::new(self.parent(key)),
            STABILITY => Arc::new(self.stability(key)),
            TESTED => Arc::new(self.tested(key)),
            _ => return None,
        })
    }
}

/// The indexes of a graph, pulled by name; what fails to be pulled reads as
/// empty and is kept as the failure.
pub struct Queried<'a> {
    queries: &'a dyn Queries,
    failed: Mutex<Option<Error>>,
}

impl<'a> Queried<'a> {
    /// The indexes `queries` answers.
    pub fn new(queries: &'a dyn Queries) -> Self {
        Self {
            queries,
            failed: Mutex::new(None),
        }
    }

    fn pull<T: Any + Default + Clone + Send + Sync>(&self, kind: &str, key: &str) -> T {
        let found = self
            .queries
            .pull(kind, key)
            .and_then(|value| {
                value
                    .downcast::<T>()
                    .map_err(|_| Error::Failed(format!("`{kind}` has an unexpected value")))
            })
            .map(|value| value.as_ref().clone());
        found.unwrap_or_else(|error| {
            self.failed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_or_insert(error);
            T::default()
        })
    }
}

impl Indexes for Queried<'_> {
    fn measured(&self, symbol: &Symbol) -> Option<Measures> {
        let file = symbol.file.to_string_lossy();
        let found = self
            .queries
            .pull(MEASURES, &subject_key(MEASURES, &file))
            .and_then(|value| {
                value
                    .downcast::<BTreeMap<SymbolId, Measures>>()
                    .map_err(|_| Error::Failed(format!("`{MEASURES}` has an unexpected value")))
            });
        match found {
            Ok(measures) => measures.get(&symbol.id).copied(),
            Err(error) => {
                self.failed
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get_or_insert(error);
                None
            }
        }
    }

    fn takers(&self, ty: &str) -> Vec<SymbolId> {
        self.pull(TAKERS, ty)
    }

    fn users(&self, id: &SymbolId) -> Vec<SymbolId> {
        self.pull(USERS, id.as_str())
    }

    fn owned(&self, test: &SymbolId) -> Vec<SymbolId> {
        self.pull(OWNED, test.as_str())
    }

    fn names(&self, name: &str) -> Vec<SymbolId> {
        self.pull(NAMES, name)
    }

    fn imports(&self, module: &str) -> Vec<String> {
        self.pull(IMPORTS, module)
    }

    fn cycle(&self, module: &str) -> Vec<String> {
        self.pull(CYCLES, module)
    }

    fn types(&self, module: &str) -> Vec<Fact> {
        self.pull(TYPES, module)
    }

    fn parent(&self, module: &str) -> Used {
        self.pull(PARENTS, module)
    }

    fn split(&self, id: &SymbolId) -> Option<Fact> {
        self.pull(SPLIT, id.as_str())
    }

    fn reach(&self, id: &SymbolId) -> Option<Reached> {
        self.pull(REACH, id.as_str())
    }

    fn stability(&self, module: &str) -> Option<Reached> {
        self.pull(STABILITY, module)
    }

    fn tested(&self, module: &str) -> bool {
        self.pull(TESTED, module)
    }

    fn credited(&self, prefix: &str, modules: &[String]) -> Arc<Credited> {
        let key = credit_key(prefix, modules);
        Arc::new(self.pull::<Credited>(CREDITED, &key))
    }

    fn failure(&self) -> Option<Error> {
        self.failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

/// The key of the tests of some modules under a prefix.
pub fn credit_key(prefix: &str, modules: &[String]) -> String {
    std::iter::once(prefix)
        .chain(modules.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\0")
}

/// The prefix and the modules a [`credit_key`] names.
pub fn credit_of(key: &str) -> (&str, Vec<String>) {
    let mut parts = key.split('\0');
    (
        parts.next().unwrap_or_default(),
        parts.map(str::to_owned).collect(),
    )
}
