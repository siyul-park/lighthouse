//! The engine, its fix orchestration and the decision tester over fixture plugins, one binary.

mod availability;
mod brief;
mod cache;
mod cache_digests;
mod cache_store;
mod engine;
mod fix;
mod fix_blame;
mod fix_incomplete;
mod fix_judge;
mod fix_kept;
mod fix_load_order;
mod fix_safety;
mod fix_write;
mod generated;
mod language_ignores;
mod models;
mod preview;
mod scope;
mod support;
mod tester;
mod tool_findings;
