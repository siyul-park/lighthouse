//! The out-of-process plugin host against fixture and real providers, one binary.

mod agent;
mod command;
mod conformance;
mod events;
mod host;
mod provider;
mod support;
mod transcript;

/// The CEL environment is installed before any test reads a decision.
#[ctor::ctor(unsafe)]
fn compose() {
    lighthouse_test_support::cel();
}
