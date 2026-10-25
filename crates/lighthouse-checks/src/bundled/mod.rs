//! The bundled plugins, derived from the bundled catalog: every pack is a
//! [`Checks`] plugin of its decisions. The `metrics` analyzer rides along.

pub(crate) mod core_pack;

use lighthouse_plugin::{Plugin, Registry};

use crate::{Checks, Composition, Error, metrics::Metrics};

/// Pack whose plugin provides the fallback text language and that the
/// `metrics` plugin is listed after.
pub(crate) const CORE: &str = "core";

/// Registry holding every bundled plugin, its decisions compiled in `comp`.
pub fn registry(comp: &Composition) -> Result<Registry, Error> {
    if !comp.is_built_in() {
        return Err(Error::invalid(
            "bundled",
            "the bundled registry needs the composition from `compose()`",
        ));
    }
    let mut registry = Registry::default();
    register(&mut registry, &lighthouse_lang_cel::Cel::new())?;
    let catalog = comp
        .bundled()
        .map_err(|e| Error::invalid("bundled", e.to_string()))?;
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
