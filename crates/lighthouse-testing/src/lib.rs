use std::sync::LazyLock;

use lighthouse_plugin::{Fixer, Plugin, PluginManifest, PresetManifest, Rule};

/// Id of the plugin and prefix of every rule it provides.
pub const ID: &str = "testing";

/// The test-contract rules of the `testing` decision pack.
pub struct Testing;

impl Plugin for Testing {
    /// The plugin id with this crate's version.
    fn manifest(&self) -> &PluginManifest {
        static MANIFEST: LazyLock<PluginManifest> = LazyLock::new(|| PluginManifest {
            id: ID.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        });
        &MANIFEST
    }

    /// Every rule of the `testing` pack, compiled from the bundled catalog.
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        lighthouse_declarative::Declarative::bundled_rules(ID)
    }

    /// The fixes of the pack's decisions, compiled from the catalog.
    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        lighthouse_declarative::Declarative::bundled_fixers(ID)
    }

    /// The standard presets over the pack's rules.
    fn presets(&self) -> Vec<PresetManifest> {
        let rules = self.rules();
        PresetManifest::standard("testing", rules.iter().map(|rule| rule.manifest()))
    }
}
