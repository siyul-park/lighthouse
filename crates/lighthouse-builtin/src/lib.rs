mod core_plugin;

use lighthouse_plugin::{Plugin, Registry};

/// Registry holding every bundled plugin.
pub fn registry() -> Registry {
    let bundled: [&dyn Plugin; 3] = [
        &core_plugin::Core,
        &lighthouse_metrics::Metrics,
        &lighthouse_design::Design,
    ];
    let mut registry = Registry::default();
    for plugin in bundled {
        registry
            .register(plugin)
            .unwrap_or_else(|e| panic!("bundled plugin `{}` is valid: {e}", plugin.manifest().id));
    }
    registry
        .validate()
        .expect("bundled analyzers form a valid DAG");
    registry
}
