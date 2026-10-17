//! The host side of mlops: the labels of a decision, the dataset they make,
//! the training of a model on it and the evaluation of a check against them.
//! What needs a project to run checks (a session) stays with the caller,
//! which hands over what the checks found.

pub mod attach;
pub mod evaluate;
pub mod labels;
pub mod metrics;
pub mod rebuild;
pub mod train;

mod artifact;
mod dataset;
