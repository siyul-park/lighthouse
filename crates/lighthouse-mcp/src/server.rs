//! The rmcp glue: handshake, tool calls and resource reads over the shared
//! operations. Each call runs on the blocking pool because checks start
//! processes and open SQLite.

use std::env;

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, ResourceContents,
        ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
};
use serde_json::Value;

use crate::{
    definitions, resources,
    tools::{Caller, call},
};

/// The reviewer id when the client names itself neither by `clientInfo` nor
/// by `LIGHTHOUSE_REVIEWER`.
const ANONYMOUS: &str = "mcp-agent";

pub struct Lighthouse;

impl ServerHandler for Lighthouse {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_instructions(
            "Lighthouse remembers this project's design decisions as rules. Call `check` after \
             changing code; fix findings or judge review-tier ones with `review_resolve`. \
             Read lighthouse://patterns/{id} for the full text of a rule.",
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(definitions::all()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let caller = Caller {
            reviewer: reviewer(&context),
        };
        let name = request.name.to_string();
        let args = request.arguments.map_or(Value::Null, Value::Object);
        let outcome = tokio::task::spawn_blocking(move || call(&name, args, &caller))
            .await
            .map_err(|e| McpError::internal_error(format!("tool panicked: {e}"), None))?;
        let result = match outcome {
            Ok(value) => CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string_pretty(&value).unwrap_or_default(),
            )]),
            Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
        };
        Ok(result.into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult::with_all_items(resources::list()))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult::with_all_items(
            resources::templates(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri;
        let wanted = uri.clone();
        let read = tokio::task::spawn_blocking(move || resources::read(&wanted))
            .await
            .map_err(|e| McpError::internal_error(format!("read panicked: {e}"), None))?;
        match read {
            Ok((text, mime)) => Ok(ReadResourceResult::new(vec![
                ResourceContents::text(text, uri).with_mime_type(mime),
            ])
            .into()),
            Err(message) => Err(McpError::resource_not_found(message, None)),
        }
    }
}

/// `LIGHTHOUSE_REVIEWER`, else the client's name from the handshake.
fn reviewer(context: &RequestContext<RoleServer>) -> String {
    env::var("LIGHTHOUSE_REVIEWER")
        .ok()
        .filter(|id| !id.is_empty())
        .or_else(|| {
            context
                .peer
                .peer_info()
                .map(|info| info.client_info.name.clone())
        })
        .unwrap_or_else(|| ANONYMOUS.to_owned())
}
