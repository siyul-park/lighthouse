//! The query database: memoised pure functions of inputs, validated by
//! revisions instead of recomputed.
//!
//! - An [`Input`] is set from outside with a [`Durability`]. A new
//!   [`Revision`] starts only when the value's hash differs.
//! - A [`Query`] is a function of a key and of what it reads through the
//!   [`Db`]. Every read is recorded with the hash seen.
//! - A pull validates a stored node, deps in order, and runs it only when a
//!   dependency moved. A run that gives an equal hash keeps `changed_at`
//!   (backdating), so what read it stays green. Nodes that read only durable
//!   inputs are green without a walk while none of them changed.
//! - The store, `graph.db`, is the cache: rows are read when pulled, values
//!   apart from hashes, and written back at [`Db::commit`]. An evicted value
//!   keeps its hash and dependencies.
//!
//! The model is Salsa's red-green algorithm and Bazel's verifying traces,
//! implemented at the size needed: Mokhov, Mitchell and Peyton Jones, *Build
//! Systems a la Carte* (ICFP 2018).

mod db;
mod dirty;
mod error;
mod explain;
mod frame;
mod node;
mod pull;
mod query;
mod revision;
mod settle;
mod shared;
mod store;

pub use db::{DEFAULT_LIMIT, Db};
pub use error::Error;
pub use explain::{Explanation, Reason, Step};
pub use query::{Input, Key, Query, Value};
pub use revision::{Durability, Revision};
