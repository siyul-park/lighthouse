//! The tool list: each tool of `lighthouse-surface` as the MCP describes it,
//! with its title and description from the command's help, its input schema
//! from the command's arguments, its output schema from the kind it returns
//! and its annotations from the table of tools.

use std::sync::Arc;

use lighthouse_session::Session;
use lighthouse_surface::{kinds, tools};
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde_json::Value;

/// Every tool, in the order of the table. A tool that may reach outside the
/// machine says so only when this project binds a remote model; when the
/// project cannot be loaded it says so as the table does.
pub fn all() -> Vec<Tool> {
    let session = Session::load_or_default(None).ok();
    let kinds = kinds::kinds();
    tools::tools()
        .iter()
        .map(|tool| {
            let (title, description) = tool.help();
            let open_world = session
                .as_ref()
                .map_or(tool.hints.open_world, |session| tool.open_world(session));
            let annotations = ToolAnnotations::new()
                .read_only(tool.hints.read_only)
                .destructive(tool.hints.destructive)
                .idempotent(tool.hints.idempotent)
                .open_world(open_world);
            Tool::new(tool.name, description, object(tool.input_schema()))
                .with_title(title)
                .with_raw_output_schema(Arc::new(object(kinds[tool.kind].clone())))
                .annotate(annotations)
        })
        .collect()
}

fn object(schema: Value) -> JsonObject {
    match schema {
        Value::Object(map) => map,
        other => unreachable!("a schema is an object, not {other}"),
    }
}
