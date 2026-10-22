//! The plugin of one pack: the checks and fixers its decisions compile into,
//! and what only code provides for it (the fallback text language of `core`).

use lighthouse_plugin::{LanguageProvider, Plugin, PluginManifest, Rule};
use lighthouse_spec::Catalog;

use crate::{
    Error,
    bundled::{CORE, core_pack},
    check::Check,
    fix::Fixer,
};

/// A plugin whose rules and fixers are the decisions of one catalog pack.
pub struct Checks {
    manifest: PluginManifest,
    checks: Vec<Check>,
    fixers: Vec<Fixer>,
    languages: fn() -> Vec<Box<dyn LanguageProvider>>,
}

impl Checks {
    /// Builds the checks and fixers of every decision in pack `id` whose check
    /// is `cel` (or another program this crate runs) or that has a `fix`.
    pub fn new(id: &str, catalog: &Catalog) -> Result<Self, Error> {
        let prefix = format!("{id}/");
        let mut checks = Vec::new();
        let mut fixers = Vec::new();
        for decision in catalog.decisions().filter(|d| d.id().starts_with(&prefix)) {
            let over = decision
                .options_from()
                .and_then(|from| catalog.decision(from));
            let decision =
                &over.map_or_else(|| decision.clone(), |source| decision.inheriting(source));
            checks.extend(Check::new(decision)?);
            fixers.extend(Fixer::new(decision)?);
        }
        let languages: fn() -> _ = if id == CORE {
            core_pack::languages
        } else {
            Vec::new
        };
        Ok(Self {
            manifest: PluginManifest {
                id: id.to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            checks,
            fixers,
            languages,
        })
    }

    /// Whether the pack has no checks.
    pub fn is_empty(&self) -> bool {
        self.checks.is_empty()
    }
}

impl Plugin for Checks {
    /// The pack id with this crate's version.
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// The language providers only code provides.
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        (self.languages)()
    }

    /// One rule per check, in catalog order.
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        self.checks
            .iter()
            .cloned()
            .map(|c| Box::new(c) as Box<dyn Rule>)
            .collect()
    }

    /// One fixer per decision of the pack that has a `fix`.
    fn fixers(&self) -> Vec<Box<dyn lighthouse_plugin::Fixer>> {
        self.fixers
            .iter()
            .cloned()
            .map(|f| Box::new(f) as Box<dyn lighthouse_plugin::Fixer>)
            .collect()
    }
}
