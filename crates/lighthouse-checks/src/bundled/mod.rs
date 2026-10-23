//! The bundled plugins, derived from the bundled catalog: every pack is a
//! [`Checks`] plugin of its decisions. The `metrics` analyzer rides along.

pub(crate) mod core_pack;

use lighthouse_plugin::{Plugin, Registry};
use lighthouse_spec::Catalog;

use crate::{Checks, metrics::Metrics};

/// Pack whose plugin provides the fallback text language and that the
/// `metrics` plugin is listed after.
pub(crate) const CORE: &str = "core";

/// Registry holding every bundled plugin.
pub fn registry() -> Registry {
    let mut registry = Registry::default();
    register(&mut registry, &lighthouse_cel::Cel::new());
    crate::compose::compose_from(&registry)
        .unwrap_or_else(|e| panic!("the bundled CEL libraries compose: {e}"));
    let catalog = Catalog::bundled();
    for pack in catalog.packs() {
        let checks = Checks::new(&pack.id, catalog)
            .unwrap_or_else(|e| panic!("bundled decisions of `{}` are valid: {e}", pack.id));
        register(&mut registry, &checks);
        if pack.id == CORE {
            register(&mut registry, &Metrics);
        }
    }
    registry
        .validate()
        .expect("bundled analyzers form a valid DAG");
    registry
}

fn register(registry: &mut Registry, plugin: &dyn Plugin) {
    registry
        .register(plugin)
        .unwrap_or_else(|e| panic!("bundled plugin `{}` is valid: {e}", plugin.manifest().id));
}
