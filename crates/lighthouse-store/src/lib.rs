//! The quality store: findings with their history in a local SQLite cache,
//! and the verdicts on them in a committed, append-only decision log
//! (`.lighthouse/decisions.jsonl`) that the cache is synchronized from.
//!
//! Labels for later learning follow [`lighthouse_model::Label`]: confirmed is
//! positive, rejected as a false positive or too broad is negative, the other
//! rejections are separate targets, deferred is unlabeled, and a finding
//! nobody reviewed has no label.

mod digest;
mod error;
mod log;
mod migrations;
mod record;
mod store;

pub use error::Error;
pub use record::{
    Filter, FindingRecord, Judgment, LatestReview, NewReview, Observed, Resolved, ReviewEvent, Run,
    RunSummary, Stamp, Standing, State, StatusFilter, Unchecked,
};
pub use store::Store;
