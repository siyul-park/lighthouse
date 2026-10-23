//! The composition of the CEL environment: the libraries of the bundled
//! plugins registered into the `cel` crate's standard environment, installed
//! once for the process (spec never knows a library), and the function
//! catalog they declare, which the load-time check of the functions a decision
//! calls, the revision of a check and the generated docs read.

use std::{collections::BTreeSet, sync::OnceLock};

use lighthouse_plugin::{Function, Registry};

static CATALOG: OnceLock<Result<Vec<Entry>, String>> = OnceLock::new();

/// A function of the catalog with the library that provides it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub library: String,
    pub version: String,
    pub function: Function,
}

/// Installs the environment of the bundled libraries, once per process: what
/// every entry point does before it reads a decision. The first call
/// decides; later calls (and `registry`) find it done.
pub fn compose() -> Result<(), String> {
    let mut registry = Registry::default();
    registry
        .register(&lighthouse_cel::Cel::new())
        .map_err(|e| format!("the `cel` plugin: {e}"))?;
    compose_from(&registry)
}

/// Like [`compose`], from the libraries of `registry`, when this is the first
/// composition of the process.
pub(crate) fn compose_from(registry: &Registry) -> Result<(), String> {
    CATALOG
        .get_or_init(|| build(registry))
        .as_ref()
        .map(drop)
        .map_err(Clone::clone)
}

/// The function catalog of the composed environment.
pub fn catalog() -> Result<&'static [Entry], String> {
    CATALOG
        .get()
        .ok_or_else(|| "the CEL environment is not composed".to_owned())?
        .as_deref()
        .map_err(Clone::clone)
}

fn build(registry: &Registry) -> Result<Vec<Entry>, String> {
    let mut env = lighthouse_spec::base()?;
    for library in registry.cel_libraries() {
        library.register(&mut env).map_err(|e| e.to_string())?;
    }
    let entries: Vec<Entry> = registry
        .cel_functions()
        .into_iter()
        .map(|c| Entry {
            library: c.library.to_owned(),
            version: c.version.to_owned(),
            function: c.function.clone(),
        })
        .collect();
    let namespaces: BTreeSet<String> = entries
        .iter()
        .filter_map(|e| e.function.namespace().map(str::to_owned))
        .collect();
    lighthouse_spec::install(env, namespaces)?;
    Ok(entries)
}
