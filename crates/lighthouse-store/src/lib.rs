//! The quality store: findings with their history in a local SQLite cache,
//! and the judgments and suppressions on them in a committed, append-only
//! decision log (`.lighthouse/decisions.jsonl`) that the cache is synchronized
//! from. The cache has one schema and no migrations: a database of another
//! version is emptied and filled again from the log.
//!
//! Labels for later learning follow [`lighthouse_model::Label`]: `fail` is
//! positive, `pass` and `notApplicable` are negative, a `fail` left in place by
//! a suppression is a target of its own, and a finding nobody judged has no
//! label.

mod digest;
mod error;
mod judged;
mod log;
mod record;
mod schema;
mod store;

use lighthouse_resource::Descriptor;

pub use error::Error;
pub use log::{JudgmentSpec, SuppressionSpec};
pub use record::{
    Filter, FindingRecord, FixEvent, JudgmentEvent, NewFix, NewJudgment, Observed, Resolved,
    Ruling, Run, RunSummary, Stamp, Standing, State, StatusFilter, Subject, SuppressionEvent,
    Unchecked,
};
pub use store::Store;

/// The JSON Schema of every record kind the decision log holds.
pub fn descriptors() -> Vec<Descriptor> {
    vec![
        Descriptor::of::<JudgmentSpec>(),
        Descriptor::of::<SuppressionSpec>(),
    ]
}
