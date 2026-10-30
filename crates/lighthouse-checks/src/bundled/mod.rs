//! The in-process plugins, derived from a catalog: every pack is a
//! [`Checks`] plugin of its decisions. The `metrics` analyzer rides along.

pub(crate) mod core_pack;

use lighthouse_plugin::{Plugin, Registry};
use lighthouse_spec::Catalog;

use crate::{Checks, Composition, Error, metrics::Metrics};

/// Pack whose plugin provides the fallback text language and that the
/// `metrics` plugin is listed after.
pub(crate) const CORE: &str = "core";

/// Registry holding `libraries` and every bundled plugin, its decisions
/// compiled in `comp`.
pub fn registry(comp: &Composition, libraries: Registry) -> Result<Registry, Error> {
    registry_of(comp, bundled(comp)?, libraries)
}

/// The ids of the bundled plugins, which a project never lists: the CEL
/// libraries, the packs of the bundled catalog and the `metrics` analyzer.
pub fn ids(comp: &Composition, libraries: &Registry) -> Result<Vec<String>, Error> {
    let mut ids: Vec<String> = libraries.manifests().iter().map(|m| m.id.clone()).collect();
    ids.extend(bundled(comp)?.packs().iter().map(|p| p.id.clone()));
    ids.push(Metrics.manifest().id.clone());
    Ok(ids)
}

/// Like [`registry`], with the plugins of the packs of `catalog`: the bundled
/// catalog, or the bundled one with the catalogs of language plugins added.
pub fn registry_of(
    comp: &Composition,
    catalog: &Catalog,
    mut registry: Registry,
) -> Result<Registry, Error> {
    if !comp.is_built_in() {
        return Err(Error::invalid(
            "bundled",
            "the bundled registry needs the composition from `compose()`",
        ));
    }
    for pack in catalog.packs() {
        let checks = Checks::new(comp, &pack.id, catalog)?;
        register(&mut registry, &checks)?;
        if pack.id == CORE {
            register(&mut registry, &Metrics)?;
        }
    }
    registry
        .validate()
        .map_err(|e| Error::invalid("bundled", format!("analyzers form no valid DAG: {e}")))?;
    Ok(registry)
}

fn register(registry: &mut Registry, plugin: &dyn Plugin) -> Result<(), Error> {
    let id = plugin.manifest().id.clone();
    registry
        .register(plugin)
        .map_err(|e| Error::invalid(&id, format!("bundled plugin is not valid: {e}")))
}

fn bundled(comp: &Composition) -> Result<&Catalog, Error> {
    comp.bundled()
        .map_err(|e| Error::invalid("bundled", e.to_string()))
}
