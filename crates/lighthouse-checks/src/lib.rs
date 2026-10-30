//! The checks of the bundled decision packs, and the machinery that compiles
//! a decision's spec into them. A `cel` check names what it selects
//! (`symbol`, `function`, `edge`, `module`, `file` or `test`), a `where`
//! expression that is true for a violation, and a message; the decision that
//! holds it supplies id, severity, scope, evidence fields and the examples
//! that `lighthouse decision test` runs. Around the declarative core sit the
//! per-function metrics the checks read and the
//! registry of every bundled plugin, derived from the catalog's packs.

mod abstain;
mod builder;
mod bundled;
pub mod caching;
mod cel_rule;
mod check;
mod checks;
pub mod command;
mod compose;
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
#[doc(hidden)]
pub mod probe;
mod table;
mod text;

pub use bundled::{ids, registry, registry_of};
pub use checks::Checks;
pub use compose::{Composition, Entry, compose, compose_from};

impl Error {
    pub(crate) fn invalid(rule: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            rule: rule.to_owned(),
            message: message.into(),
        }
    }
}

/// Why a check could not be built.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{rule}: {message}")]
    Invalid { rule: String, message: String },
    #[error(transparent)]
    Spec(#[from] lighthouse_spec::Error),
}
