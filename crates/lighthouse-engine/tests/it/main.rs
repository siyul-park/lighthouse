//! The engine, its fix orchestration and the decision tester over fixture plugins, one binary.

mod availability;
mod brief;
mod cache;
mod cache_digests;
mod cache_store;
mod engine;
mod fix;
mod fix_judge;
mod fix_kept;
mod fix_safety;
mod fix_write;
mod models;
mod preview;
mod scope;
mod support;
mod tester;
mod tool_findings;

/// The CEL environment is installed before any test reads a decision.
#[ctor::ctor(unsafe)]
fn compose() {
    lighthouse_test_support::cel();
}
