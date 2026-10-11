//! The MCP frontend: `lighthouse mcp` serves the commands of
//! `crate::surface` to a coding agent over stdio, as tools with JSON
//! schemas, and the catalog as resources. Every judgment recorded here is
//! attributed to a `SoftwareAgent`.

mod definitions;
mod resources;
mod server;

use std::error::Error;

use rmcp::{ServiceExt, transport::stdio};
use server::Lighthouse;

/// Serves MCP on stdin and stdout until the client disconnects. The project is
/// the one found upward from the current directory, looked up on every call
/// so that edits to its config and decisions take effect at once.
pub fn serve() -> Result<(), Box<dyn Error + Send + Sync>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let running = Lighthouse.serve(stdio()).await?;
        running.waiting().await?;
        Ok(())
    })
}
