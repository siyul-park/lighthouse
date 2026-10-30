//! OpenAI's API (https://platform.openai.com/docs/api-reference). One
//! function per operation used, named by its operationId in snake_case. The
//! types are `async-openai`'s, generated from OpenAI's OpenAPI spec; only its
//! type features are enabled, and the HTTP is this crate's [`Client`].

use async_openai::types::chat::{CreateChatCompletionRequest, CreateChatCompletionResponse};
use lighthouse_protocol::ErrorObject;
use serde_json::{Map, Value};

use crate::{endpoint::Endpoint, http::Client};

/// `createChatCompletion`: `POST /chat/completions`. `fixed` holds fields the
/// request type cannot carry; the body is the request, then `fixed`, then the
/// endpoint's `body`, each overriding the one before. A refusal comes
/// back as the error body the service answered; an error body in a 200
/// response is carried the same way.
pub fn create_chat_completion(
    client: &dyn Client,
    endpoint: &Endpoint,
    request: &CreateChatCompletionRequest,
    fixed: Map<String, Value>,
) -> Result<CreateChatCompletionResponse, ErrorObject> {
    let mut body = serde_json::to_value(request).map_err(|e| e.to_string())?;
    if let Some(fields) = body.as_object_mut() {
        fields.extend(fixed);
        fields.extend(endpoint.body.clone());
    }
    let response = client.post(
        &endpoint.url("chat/completions"),
        &endpoint.headers()?,
        &body,
        endpoint.timeout,
    )?;
    if let Some(error) = response.get("error").filter(|e| !e.is_null()) {
        return Err(ErrorObject::with_data(
            "the provider answered with an error",
            error.clone(),
        ));
    }
    serde_json::from_value(response)
        .map_err(|e| format!("the response does not fit createChatCompletion's: {e}").into())
}

#[cfg(test)]
mod tests;
