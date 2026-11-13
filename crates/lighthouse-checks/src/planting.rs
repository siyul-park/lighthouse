//! The local decisions an example writes in its own files, made rules: the
//! way a decision about decisions is shown on decisions of its own.

use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Files};

use crate::{Checks, Composition};

/// `catalog` with the local decisions in `files` over it, and their rules
/// registered in `registry`. `registry` must hold no plugin `local` yet.
pub fn planted(
    comp: &Composition,
    mut registry: Registry,
    catalog: &Catalog,
    files: Files,
) -> Result<(Registry, Catalog), String> {
    let local = Catalog::from_local(files, comp.cel()).map_err(|e| e.to_string())?;
    with_local(comp, &mut registry, &local)?;
    let catalog = Catalog::overlay(catalog, &local).map_err(|e| e.to_string())?;
    Ok((registry, catalog))
}

/// Registers the rules of the local decisions of `local` in `registry` as the
/// plugin `local`, when there are any. `registry` must hold no plugin `local`
/// yet.
pub fn with_local(
    comp: &Composition,
    registry: &mut Registry,
    local: &Catalog,
) -> Result<(), String> {
    let rules = Checks::new(comp, "local", local).map_err(|e| e.to_string())?;
    if rules.is_empty() {
        return Ok(());
    }
    registry.register(&rules).map_err(|e| e.to_string())
}
