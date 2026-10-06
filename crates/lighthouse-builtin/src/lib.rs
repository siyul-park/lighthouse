mod core_plugin;

use lighthouse_plugin::Registry;

/// Registry holding every bundled plugin.
pub fn registry() -> Registry {
    let mut registry = Registry::default();
    registry
        .register(&core_plugin::Core)
        .expect("bundled plugin `core` is valid");
    registry
        .validate()
        .expect("bundled analyzers form a valid DAG");
    registry
}
