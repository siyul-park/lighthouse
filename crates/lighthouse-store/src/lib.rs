//! The quality store: findings with their history and the append-only log of
//! review verdicts, in one SQLite file per project.
//!
//! Labels for later learning follow [`lighthouse_model::Label`]: confirmed is
//! positive, rejected as a false positive or too broad is negative, the other
//! rejections are separate targets, deferred is unlabeled, and a finding
//! nobody reviewed has no label.

mod error;
mod migrations;
mod record;
mod store;

pub use error::Error;
pub use record::{
    Filter, FindingRecord, LatestReview, NewReview, Observed, ReviewEvent, Run, RunSummary,
    StatusFilter,
};
pub use store::Store;
