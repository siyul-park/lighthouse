//! The rmcp glue: handshake, tool calls and resource reads over the shared
//! operations. Each call runs on the blocking pool because checks start
//! processes and open SQLite.

use crate::surface::{Context, Reply, reply::Body, tools};
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

use crate::mcp::{definitions, resources};

/// The agent id when the client names itself by `clientInfo` and
/// `LIGHTHOUSE_ATTRIBUTED_TO` says nothing.
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
            "Lighthouse remembers this project's design decisions and enforces them as rules. \
             Call `check` after changing code; fix findings (`fix` does it for decisions marked \
             fixable, `dryRun` first) or judge the ones that ask for review with \
             `review_list` and `review_resolve`. Read lighthouse://decisions/{id} for the full \
             text of a decision.",
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
        let ctx = Context::agent(agent(&context));
        let name = request.name.to_string();
        let args = request.arguments.map_or(Value::Null, Value::Object);
        let outcome = tokio::task::spawn_blocking(move || {
            let outcome = tools::call(&ctx, &name, args);
            (name, outcome)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("tool panicked: {e}"), None))?;
        let result = match outcome {
            (name, None) => {
                return Err(McpError::invalid_params(
                    format!("unknown tool `{name}`"),
                    None,
                ));
            }
            (
                _,
                Some(Ok(Reply {
                    body: Body::Json(value),
                    ..
                })),
            ) => CallToolResult::structured(value),
            (
                _,
                Some(Ok(Reply {
                    body: Body::Text(text),
                    ..
                })),
            ) => CallToolResult::success(vec![ContentBlock::text(text)]),
            (_, Some(Err(e))) => CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
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

/// The client's name from the handshake: the agent a judgment is attributed
/// to unless `attributedTo` says otherwise (`LIGHTHOUSE_ATTRIBUTED_TO` is its
/// default, as for the command line).
fn agent(context: &RequestContext<RoleServer>) -> String {
    context
        .peer
        .peer_info()
        .map(|info| info.client_info.name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| ANONYMOUS.to_owned())
}
