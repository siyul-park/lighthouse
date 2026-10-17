//! The commands an agent also uses, defined once for both frontends. The clap
//! tree (`commands`), the argument structs that are both clap flags and MCP
//! tool arguments (`args`), the operations behind them and the tool table that
//! derives the schemas of what they print (`tools`) live here; the terminal
//! (`crate::terminal`) parses and prints, the MCP server (`crate::mcp`) wraps
//! the same operations as tools. Operations return a `Reply` (body, notes for
//! stderr, exit code) and never print.

pub mod args;
mod check;
pub mod commands;
mod decision;
mod list;
mod model;
pub mod reply;
mod review;
pub mod tools;

pub use commands::{Action, run};
pub use reply::{Caller, Context, Reply};

pub use lighthouse_model::Result;
