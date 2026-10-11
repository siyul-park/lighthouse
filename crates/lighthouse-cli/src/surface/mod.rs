//! The commands an agent also uses, defined once for both frontends. The clap
//! tree (`cli`), the argument structs that are both clap flags and MCP tool
//! arguments (`args`), the operations behind them and the JSON they print
//! (`kinds`) live here; the terminal (`crate::cli`) parses and prints, the MCP
//! server (`crate::mcp`) wraps the same operations as tools. Operations return
//! a `Reply` (stdout, JSON value, notes for stderr, exit code) and never print.

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
