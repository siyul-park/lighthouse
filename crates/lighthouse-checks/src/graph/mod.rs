//! The facts of the code model on the graph: a view of the model that reads
//! what it needs from the graph's parts, and the queries that make the indexes
//! and the facts of subjects from it.

mod kinds;
#[cfg(test)]
mod tests;

pub(crate) use kinds::Pulls;
