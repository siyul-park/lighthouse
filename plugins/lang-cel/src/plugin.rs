//! The plugin: its id, its libraries and its language provider.

use lighthouse_plugin::{CelLibrary, Plugin, PluginManifest};

use crate::{language, libraries};

/// The plugin id, and the prefix of its libraries' ids.
pub const ID: &str = "lang-cel";

/// The plugin that provides the libraries.
pub struct Cel {
    manifest: PluginManifest,
}

impl Cel {
    /// The plugin, with the version of this crate.
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: ID.to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
        }
    }
}

impl Default for Cel {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for Cel {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn languages(&self) -> Vec<Box<dyn lighthouse_plugin::LanguageProvider>> {
        vec![Box::new(language::Language::new())]
    }

    fn cel_libraries(&self) -> Vec<Box<dyn CelLibrary>> {
        libraries::all()
    }
}
