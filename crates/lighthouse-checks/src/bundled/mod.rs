//! The bundled plugins, derived from the bundled catalog: every pack is a
//! [`Declarative`] plugin of its decisions, plus what only code provides for
//! it (the fallback text language of `core`, the order keys of `design`). The `metrics` analyzers ride along.

mod core_pack;

use lighthouse_plugin::{LanguageProvider, OrderKey, Plugin, PluginManifest, Registry, Rule};
use lighthouse_spec::Catalog;

use crate::{Declarative, metrics::Metrics, order};

/// Pack whose plugin the `metrics` plugin is listed after.
const CORE: &str = "core";
/// Pack whose plugin provides the order keys.
const DESIGN: &str = "design";

/// A bundled pack: its declarative decisions and the code that goes with them.
pub struct Pack {
    decisions: Declarative,
    code: Code,
}

/// What only code provides for a pack.
struct Code {
    languages: fn() -> Vec<Box<dyn LanguageProvider>>,
    order_keys: fn() -> Vec<Box<dyn OrderKey>>,
}

impl Code {
    const NONE: Self = Self {
        languages: Vec::new,
        order_keys: Vec::new,
    };
}

impl Pack {
    /// Compiles pack `id` of the bundled catalog. Panics when a bundled
    /// decision does not compile: that is tested.
    pub fn of(id: &str, catalog: &Catalog) -> Self {
        let decisions = Declarative::from_catalog(id, catalog)
            .unwrap_or_else(|e| panic!("bundled decisions of `{id}` are valid: {e}"));
        let code = match id {
            CORE => Code {
                languages: core_pack::languages,
                ..Code::NONE
            },
            DESIGN => Code {
                order_keys: order::keys,
                ..Code::NONE
            },
            _ => Code::NONE,
        };
        Self { decisions, code }
    }
}

impl Plugin for Pack {
    /// The pack's id and version.
    fn manifest(&self) -> &PluginManifest {
        self.decisions.manifest()
    }

    /// The language providers only code provides.
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        (self.code.languages)()
    }

    /// The rules of the pack's decisions.
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        self.decisions.rules()
    }

    /// One fixer per decision of the pack that has a `fix`.
    fn fixers(&self) -> Vec<Box<dyn lighthouse_plugin::Fixer>> {
        self.decisions.fixers()
    }

    /// The keys fixes of the pack sort by.
    fn order_keys(&self) -> Vec<Box<dyn OrderKey>> {
        (self.code.order_keys)()
    }
}

/// Registry holding every bundled plugin.
pub fn registry() -> Registry {
    let catalog = Catalog::bundled();
    let mut registry = Registry::default();
    for pack in &catalog.packs {
        register(&mut registry, &Pack::of(&pack.id, catalog));
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
