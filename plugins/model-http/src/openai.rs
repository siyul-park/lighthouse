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
mod tests {
    use async_openai::types::chat::{
        ChatCompletionRequestMessage, ChatCompletionRequestUserMessage,
    };
    use serde_json::json;

    use super::*;
    use crate::recording::Recording;

    /// The response of the Chat Completions reference.
    const COMPLETION: &str = r#"{"id":"chatcmpl-123","object":"chat.completion","created":1700000000,"model":"gpt-5.2","choices":[{"index":0,"message":{"role":"assistant","content":"Hello! How can I help you today?"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":12,"total_tokens":22},"service_tier":"auto","system_fingerprint":"fp_123","moderation":null}"#;

    fn request() -> CreateChatCompletionRequest {
        CreateChatCompletionRequest {
            model: "gpt-5.2".to_owned(),
            messages: vec![ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessage::from("Hello"),
            )],
            ..Default::default()
        }
    }

    #[test]
    fn it_posts_to_chat_completions_and_reads_the_documented_response() {
        let client = Recording::replying(vec![serde_json::from_str(COMPLETION).unwrap()]);
        let endpoint = Endpoint::at("openai", "https://api.openai.com/v1/");
        let response = create_chat_completion(&client, &endpoint, &request(), Map::new()).unwrap();
        assert_eq!(
            response.choices[0].message.content.as_deref(),
            Some("Hello! How can I help you today?")
        );
        let asked = client.asked();
        assert_eq!(asked[0].0, "https://api.openai.com/v1/chat/completions");
        assert_eq!(asked[0].1["model"], "gpt-5.2");
        assert_eq!(asked[0].1["messages"][0]["role"], "user");
    }

    #[test]
    fn body_overrides_the_request_and_the_fixed_fields() {
        let client = Recording::replying(vec![serde_json::from_str(COMPLETION).unwrap()]);
        let mut endpoint = Endpoint::at("openai", "https://x");
        endpoint.body = json!({"max_tokens": 1024, "provider": {"a": 1}, "model": "other"})
            .as_object()
            .unwrap()
            .clone();
        let fixed = json!({"max_tokens": 1}).as_object().unwrap().clone();
        create_chat_completion(&client, &endpoint, &request(), fixed).unwrap();
        let body = &client.asked()[0].1;
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["provider"]["a"], 1);
        assert_eq!(body["model"], "other");
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn an_error_body_is_carried_as_data() {
        let error = json!({"message": "rate limited", "type": "requests"});
        let client = Recording::replying(vec![json!({ "error": error })]);
        let endpoint = Endpoint::at("openai", "https://x");
        let failed =
            create_chat_completion(&client, &endpoint, &request(), Map::new()).unwrap_err();
        assert_eq!(failed.data, Some(error));
    }

    #[test]
    fn a_response_of_another_shape_fails() {
        let client = Recording::replying(vec![Value::Null]);
        let endpoint = Endpoint::at("openai", "https://x");
        assert!(create_chat_completion(&client, &endpoint, &request(), Map::new()).is_err());
    }
}
