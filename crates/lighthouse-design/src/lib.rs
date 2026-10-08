mod order;

use std::sync::LazyLock;

use lighthouse_plugin::{Fixer, OrderKey, Plugin, PluginManifest, PresetManifest, Rule};

/// Id of the plugin and prefix of every rule it provides.
pub const ID: &str = "design";

/// The design rules plugin: the rules of the bundled `design` pack.
pub struct Design;

impl Plugin for Design {
    /// The plugin id with this crate's version.
    fn manifest(&self) -> &PluginManifest {
        static MANIFEST: LazyLock<PluginManifest> = LazyLock::new(|| PluginManifest {
            id: ID.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        });
        &MANIFEST
    }

    /// Every rule of the pack, compiled from the bundled catalog.
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        lighthouse_declarative::Declarative::bundled_rules(ID)
    }

    /// The fixes of the pack's decisions, compiled from the catalog.
    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        lighthouse_declarative::Declarative::bundled_fixers(ID)
    }

    /// The keys `reorder` fixes sort by.
    fn order_keys(&self) -> Vec<Box<dyn OrderKey>> {
        vec![Box::new(order::GroupKey), Box::new(order::ConstructorKey)]
    }

    /// The standard presets over the pack's rules.
    fn presets(&self) -> Vec<PresetManifest> {
        let rules = self.rules();
        PresetManifest::standard(ID, rules.iter().map(|rule| rule.manifest()))
    }
}
