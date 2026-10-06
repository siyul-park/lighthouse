//! Declarative rules: a CEL expression over the code model. A rule file names
//! what it selects (`symbol`, `function`, `edge`, `module`, `file` or `test`),
//! a `where` expression that is true for a violation, and a message; the
//! pattern that names the file supplies id, severity, scope, evidence fields
//! and the examples that `lighthouse rule test` runs.

mod definition;
mod facts;
mod local;
mod rule;

use lighthouse_plugin::{Manifest, Plugin, Rule};
use lighthouse_spec::{Catalog, Implementation};

pub use local::load_local;
use rule::DeclarativeRule;

/// Why a declarative rule could not be built.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{rule}: {message}")]
    Invalid { rule: String, message: String },
    #[error(transparent)]
    Spec(#[from] lighthouse_spec::Error),
    #[error("{path}: {message}")]
    Io { path: String, message: String },
}

impl Error {
    pub(crate) fn invalid(rule: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            rule: rule.to_owned(),
            message: message.into(),
        }
    }
}

/// A plugin whose rules are the declarative patterns of one catalog pack.
pub struct Declarative {
    id: String,
    definitions: Vec<DeclarativeRule>,
}

impl Declarative {
    /// Builds the rules of every pattern in pack `id` whose implementation is
    /// declarative.
    pub fn from_catalog(id: &str, catalog: &Catalog) -> Result<Self, Error> {
        let mut definitions = Vec::new();
        for pattern in catalog
            .patterns()
            .filter(|p| p.id.starts_with(&format!("{id}/")))
        {
            let Some(Implementation::Declarative(path)) = &pattern.implementation else {
                continue;
            };
            let text = catalog
                .declarative(path)
                .ok_or_else(|| Error::invalid(&pattern.id, format!("`{path}` is not loaded")))?;
            definitions.push(DeclarativeRule::new(pattern, text)?);
        }
        Ok(Self {
            id: id.to_owned(),
            definitions,
        })
    }

    /// Whether the pack has no declarative rules.
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
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
    fn manifest(&self) -> Manifest {
        Manifest {
            id: self.id.clone(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// One rule per declarative pattern, in catalog order.
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        self.definitions
            .iter()
            .cloned()
            .map(|r| Box::new(r) as Box<dyn Rule>)
            .collect()
    }

    /// The standard presets over this pack's rules.
    fn presets(&self) -> Vec<lighthouse_plugin::Preset> {
        lighthouse_plugin::Preset::standard(&self.id, self.definitions.iter().map(|r| r.meta()))
    }
}
