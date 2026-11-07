//! The query database: memoised pure functions of inputs, validated by
//! revisions instead of recomputed.
//!
//! - An [`Input`] is set from outside with a [`Durability`]. A new
//!   [`Revision`] starts only when the value's hash differs.
//! - A [`Query`] is a function of a key and of what it reads through the
//!   [`Graph`]. Every read is recorded with the hash seen.
//! - A pull validates a stored node, deps in order, and runs it only when a
//!   dependency moved. A run that gives an equal hash keeps `changed_at`
//!   (backdating), so what read it stays green. Nodes that read only durable
//!   inputs are green without a walk while none of them changed.
//! - A node keeps its last [`RETAINED`] values with what each read. A node
//!   whose dependencies moved takes back one whose dependencies all hold
//!   again, an edit and its revert, without running.
//! - The store, `graph.db`, is the cache: rows are read when pulled, values
//!   apart from hashes, and written back at [`Graph::commit`]. An evicted value
//!   keeps its hash and dependencies.
//!
//! The model is Salsa's red-green algorithm and Bazel's verifying traces,
//! implemented at the size needed: Mokhov, Mitchell and Peyton Jones, *Build
//! Systems a la Carte* (ICFP 2018).

mod blob;
mod dirty;
mod error;
mod explain;
mod fan;
mod frame;
mod graph;
mod node;
mod pull;
mod query;
mod retained;
mod revision;
mod settle;
mod shared;
mod store;

pub use error::Error;
pub use explain::{Explanation, Reason, Step};
pub use graph::{DEFAULT_LIMIT, DEFAULT_RETAIN, Graph, Options, VERSION};
pub use node::RETAINED;
pub use query::{Input, Key, Query, Value, digest};
pub use revision::{Durability, Revision};
