//! The decision catalog, its checks, fixes and documents, one binary.

mod catalog;
mod checks;
mod clean_code;
mod environment;
mod events;
mod fixes;
mod harness_free;
mod kinds;
mod kinds_option;
mod layers;
mod local;
mod models;
mod pack_variables;
mod project;
mod reference;
mod sources;
mod validation;
mod versions;

/// The CEL environment is installed before any test reads a decision.
#[ctor::ctor(unsafe)]
fn compose() {
    lighthouse_test_support::cel();
}
