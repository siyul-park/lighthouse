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
mod cel_rule;
mod check;
mod checks;
mod compose;
mod error;
mod eval;
mod facts;
mod fix;
mod glob;
mod library;
mod manifest;
mod modules;
mod naming;
mod table;
mod text;

pub mod caching;
pub mod command;
pub mod layout;
pub mod metrics;
#[doc(hidden)]
pub mod probe;

pub use bundled::{ids, registry, registry_of};
pub use checks::Checks;
pub use compose::{Composition, Entry, compose, compose_from};
pub use error::Error;
