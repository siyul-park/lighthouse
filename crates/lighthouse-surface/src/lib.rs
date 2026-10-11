#![doc = include_str!("../README.md")]

pub mod args;
mod check;
pub mod cli;
mod decision;
mod eval;
mod fix;
pub mod kinds;
mod list;
mod model;
pub mod reply;
mod review;
mod timings;
pub mod tools;

pub use cli::{Action, run};
pub use reply::{Caller, Context, Reply};

/// Errors cross threads: the MCP server runs operations on a blocking pool.
pub type Result<T> = lighthouse_session::Result<T>;
