//! `lighthouse/chat`: the body of OpenAI's Chat Completions
//! (`createChatCompletion`), which any chat model understands, answered with
//! its response. It is how a model generates text; unlike classification it
//! gives no probability.

pub use async_openai::types::chat::{CreateChatCompletionRequest, CreateChatCompletionResponse};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Params of `lighthouse/chat`: OpenAI's request body, verbatim, and the
/// provider's `options`. `model` is the part of the reference after the
/// provider, as in the other model methods.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    #[serde(flatten)]
    pub body: CreateChatCompletionRequest,
    /// Provider options, omitted when empty and stripped by HTTP providers.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,
}

/// The chat method with its params and result. Exists to anchor the schema:
/// OpenAI's types are not described here, its API reference is.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChatCall {
    #[schemars(schema_with = "request_schema")]
    pub params: ChatRequest,
    #[schemars(schema_with = "response_schema")]
    pub result: CreateChatCompletionResponse,
}

fn request_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "description": "The body of OpenAI's createChatCompletion (POST /v1/chat/completions, https://platform.openai.com/docs/api-reference/chat/create), and `options`.",
        "required": ["model", "messages"],
        "properties": {
            "model": {"type": "string"},
            "messages": {"type": "array"},
            "options": {"type": "object"}
        }
    })
}

fn response_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "description": "OpenAI's chat completion object (https://platform.openai.com/docs/api-reference/chat/object).",
        "required": ["id", "object", "created", "model", "choices"],
        "properties": {
            "choices": {"type": "array"}
        }
    })
}
