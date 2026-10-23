//! The host side of mlops, one binary.

mod attach;
mod cross_fit;
mod evaluate;
mod labels;
mod layer;
mod layer_support;
mod metrics;
mod rebuild;
mod train;

/// The CEL environment is installed before any test reads a decision.
#[ctor::ctor(unsafe)]
fn compose() {
    lighthouse_test_support::cel();
}
