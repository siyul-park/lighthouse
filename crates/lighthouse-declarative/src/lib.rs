//! Declarative rules: the checks a decision's spec describes, compiled into
//! rules. A `cel` check
//! names what it selects (`symbol`, `function`, `edge`, `module`, `file` or
//! `test`), a `where` expression that is true for a violation, and a message;
//! the decision that holds it supplies id, severity, scope, evidence fields
//! and the examples that `lighthouse decision test` runs.

mod builder;
mod cel_rule;
mod command;
mod eval;
mod facts;
mod fix;
pub mod layout;
mod library;
mod local;
mod naming;
mod ops;
mod rule;
mod text;

use fix::SpecFixer;
use lighthouse_plugin::{Fixer, Plugin, PluginManifest, Rule};
use lighthouse_spec::Catalog;

pub use local::{load_local, local_dir, local_files};
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
    #[error("{path}: {message}")]
    Io { path: String, message: String },
    #[error("{path}: a format from before the resource model; run `lighthouse spec migrate`")]
    Legacy { path: String },
}

impl Declarative {
    /// Builds the rules of every decision in pack `id` whose check is `cel`.
    pub fn from_catalog(id: &str, catalog: &Catalog) -> Result<Self, Error> {
        let mut definitions = Vec::new();
        for decision in catalog
            .decisions()
            .filter(|d| d.id().starts_with(&format!("{id}/")))
        {
            definitions.extend(DeclarativeRule::new(decision)?);
        }
        let mut fixers = Vec::new();
        for decision in catalog
            .decisions()
            .filter(|d| d.id().starts_with(&format!("{id}/")))
        {
            fixers.extend(SpecFixer::new(decision)?.map(std::sync::Arc::new));
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

    /// The fixers of the bundled catalog's pack `id`: one per decision with a
    /// `fix`, whatever implements its rule. Panics when a bundled fix does not
    /// compile: it is tested.
    pub fn bundled_fixers(id: &str) -> Vec<Box<dyn Fixer>> {
        Self::from_catalog(id, Catalog::bundled())
            .unwrap_or_else(|e| panic!("bundled fixes are valid: {e}"))
            .fixers()
    }

    /// The declarative rules of the bundled catalog's pack `id`. Panics when
    /// the bundled catalog has a rule that does not compile: it is tested.
    pub fn bundled_rules(id: &str) -> Vec<Box<dyn Rule>> {
        Self::from_catalog(id, Catalog::bundled())
            .unwrap_or_else(|e| panic!("bundled declarative rules are valid: {e}"))
            .rules()
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
            .map(|f| Box::new(Shared(f)) as Box<dyn Fixer>)
            .collect()
    }

    /// The standard presets over this pack's rules.
    fn presets(&self) -> Vec<lighthouse_plugin::PresetManifest> {
        lighthouse_plugin::PresetManifest::standard(
            &self.manifest.id,
            self.definitions
                .iter()
                .map(|r| r.manifest())
                .filter(|m| m.enforced),
        )
    }
}

/// A plugin whose rules are the declarative decisions of one catalog pack.
pub struct Declarative {
    manifest: PluginManifest,
    definitions: Vec<DeclarativeRule>,
    fixers: Vec<std::sync::Arc<SpecFixer>>,
}

/// A fixer shared between the plugin and the registry.
struct Shared(std::sync::Arc<SpecFixer>);

impl Fixer for Shared {
    fn manifest(&self) -> &lighthouse_plugin::FixerManifest {
        self.0.manifest()
    }

    fn fix(
        &self,
        request: &lighthouse_plugin::FixRequest,
    ) -> Result<lighthouse_model::FixOutcome, lighthouse_plugin::Error> {
        self.0.fix(request)
    }
}
