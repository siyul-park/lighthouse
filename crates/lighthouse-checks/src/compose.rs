//! The composition of the CEL environment: the libraries of the bundled
//! plugins registered into the `cel` crate's standard environment, and the
//! function catalog they declare, which the load-time check of the functions a
//! decision calls, the revision of a check and the generated docs read. The
//! composition root builds one [`Composition`] and passes it to everything
//! that compiles or runs an expression; nothing keeps it in a global.

use std::{
    collections::BTreeSet,
    sync::{Arc, OnceLock},
};

use lighthouse_plugin::{Function, Plugin, Registry};
use lighthouse_spec::{Catalog, Cel};

use crate::Error;

/// A function of the catalog with the library that provides it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub library: String,
    pub version: String,
    pub function: Function,
}

/// The environment of the bundled libraries and the catalog of their
/// functions. Clones share both.
#[derive(Clone)]
pub struct Composition {
    cel: Cel,
    functions: Arc<[Entry]>,
    bundled: Arc<OnceLock<Result<Catalog, String>>>,
    built_in: bool,
}

impl Composition {
    /// The environment.
    pub fn cel(&self) -> &Cel {
        &self.cel
    }

    /// The catalog compiled into the binary, validated against the
    /// environment once.
    pub fn bundled(&self) -> Result<&Catalog, Error> {
        self.bundled
            .get_or_init(|| Catalog::embedded(&self.cel).map_err(|e| e.to_string()))
            .as_ref()
            .map_err(|why| Error::invalid("the bundled catalog", why.as_str()))
    }

    /// The function catalog of the libraries, sorted by name.
    pub fn functions(&self) -> &[Entry] {
        &self.functions
    }

    /// Whether this was composed from the bundled library registry and can
    /// therefore build the matching bundled plugin registry.
    pub(crate) fn is_built_in(&self) -> bool {
        self.built_in
    }
}

/// The composition of the bundled libraries: what every entry point builds
/// before it reads a decision.
pub fn compose() -> Result<Composition, String> {
    compose_with(&libraries()?, true)
}

/// A registry holding the plugin that provides the bundled libraries.
pub(crate) fn libraries() -> Result<Registry, String> {
    let plugin = lang_cel::Cel::new();
    let mut registry = Registry::default();
    registry
        .register(&plugin)
        .map_err(|e| format!("the `{}` plugin: {e}", plugin.manifest().id))?;
    Ok(registry)
}

/// The composition of the libraries `registry` holds.
pub fn compose_from(registry: &Registry) -> Result<Composition, String> {
    compose_with(registry, false)
}

fn compose_with(registry: &Registry, built_in: bool) -> Result<Composition, String> {
    let mut env = lighthouse_spec::base()?;
    for library in registry.cel().libraries() {
        let id = &library.manifest().id;
        library
            .register(&mut env)
            .map_err(|e| format!("CEL library `{id}`: {e}"))?;
    }
    let functions: Vec<Entry> = registry
        .cel()
        .functions()
        .into_iter()
        .map(|c| Entry {
            library: c.library.to_owned(),
            version: c.version.to_owned(),
            function: c.function.clone(),
        })
        .collect();
    let namespaces: BTreeSet<String> = functions
        .iter()
        .filter_map(|e| e.function.namespace().map(str::to_owned))
        .collect();
    Ok(Composition {
        cel: Cel::new(env, namespaces),
        functions: functions.into(),
        bundled: Arc::default(),
        built_in,
    })
}
