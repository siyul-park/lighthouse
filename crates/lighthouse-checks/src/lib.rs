//! The checks of the bundled decision packs, and the machinery that compiles
//! a decision's spec into them. A `cel` check names what it selects
//! (`symbol`, `function`, `edge`, `module`, `file` or `test`), a `where`
//! expression that is true for a violation, and a message; the decision that
//! holds it supplies id, severity, scope, evidence fields and the examples
//! that `lighthouse decision test` runs. Around the declarative core sit the
//! per-function metrics the checks read, the order keys fixes sort by, and the
//! registry of every bundled plugin, derived from the catalog's packs.

mod builder;
mod bundled;
mod cel_rule;
pub mod command;
mod eval;
mod facts;
mod fix;
mod glob;
pub mod layout;
mod library;
mod manifest;
pub mod metrics;
mod modules;
mod naming;
mod ops;
mod order;
#[doc(hidden)]
pub mod probe;
pub mod reach;
mod rule;
mod table;
mod text;

use fix::SpecFixer;
use lighthouse_plugin::{Fixer, Plugin, PluginManifest, Rule};
use lighthouse_spec::Catalog;

pub use bundled::{Pack, registry};
use rule::DeclarativeRule;

impl Error {
    pub(crate) fn invalid(rule: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            rule: rule.to_owned(),
            message: message.into(),
        }
    }
}

/// Why a declarative rule could not be built.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{rule}: {message}")]
    Invalid { rule: String, message: String },
    #[error(transparent)]
    Spec(#[from] lighthouse_spec::Error),
}

impl Declarative {
    /// Builds the rules and fixes of every decision in pack `id` whose check
    /// is `cel` (or another program this crate runs) or that has a `fix`.
    pub fn from_catalog(id: &str, catalog: &Catalog) -> Result<Self, Error> {
        let prefix = format!("{id}/");
        let mut definitions = Vec::new();
        let mut fixers = Vec::new();
        for decision in catalog.decisions().filter(|d| d.id().starts_with(&prefix)) {
            definitions.extend(DeclarativeRule::new(decision)?);
            fixers.extend(SpecFixer::new(decision)?);
        }
        Ok(Self {
            manifest: PluginManifest {
                id: id.to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            definitions,
            fixers,
        })
    }

    /// Whether the pack has no declarative rules.
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

impl Plugin for Declarative {
    /// The pack id with this crate's version.
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// One rule per declarative decision, in catalog order.
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        self.definitions
            .iter()
            .cloned()
            .map(|r| Box::new(r) as Box<dyn Rule>)
            .collect()
    }

    /// One fixer per decision of the pack that has a `fix`.
    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        self.fixers
            .iter()
            .cloned()
            .map(|f| Box::new(f) as Box<dyn Fixer>)
            .collect()
    }
}

/// A plugin whose rules are the declarative decisions of one catalog pack.
pub struct Declarative {
    manifest: PluginManifest,
    definitions: Vec<DeclarativeRule>,
    fixers: Vec<SpecFixer>,
}
