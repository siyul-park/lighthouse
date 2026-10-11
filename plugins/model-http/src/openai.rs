//! The `openai` spec: Chat Completions. The answer's shape is asked for with
//! `response_format` `json_schema` (OpenAI and OpenRouter document it:
//! https://openrouter.ai/docs/features/structured-outputs). A provider that
//! does not support it refuses the request; the request is then sent again
//! without it, and the text is parsed as strict JSON.

use lighthouse_protocol::{ClassifyParams, ClassifyResult};
use serde_json::{Value, json};

use crate::{
    http,
    serve::{Endpoint, Prompt, Transport},
};

/// Chat Completions over HTTP.
pub struct Http;

impl Transport for Http {
    fn complete(&self, endpoint: &Endpoint, prompt: &Prompt) -> Result<String, String> {
        let key = http::key(endpoint.key_env.as_deref())?;
        let url = format!(
            "{}/chat/completions",
            endpoint.base_url.trim_end_matches('/')
        );
        let post = |structured: bool| {
            http::post(
                &url,
                key.as_deref(),
                &request(&endpoint.model, prompt, structured),
            )
        };
        let response = match post(true) {
            Ok(response) => response,
            // A service that does not take `response_format` is asked again
            // without it.
            Err(ureq::Error::StatusCode(400 | 404 | 422)) => {
                post(false).map_err(|e| e.to_string())?
            }
            Err(e) => return Err(e.to_string()),
        };
        text_of(&response)
    }

    fn systemone(&self, _: &Endpoint, _: &ClassifyParams) -> Result<ClassifyResult, String> {
        Err("this spec does not speak System One".to_owned())
    }
}

/// The body of a Chat Completions request.
pub fn request(model: &str, prompt: &Prompt, structured: bool) -> Value {
    let mut body = json!({
        "model": model,
        "temperature": 0,
        "messages": [
            {"role": "system", "content": prompt.system},
            {"role": "user", "content": prompt.user},
        ],
    });
    if structured {
        body["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {"name": "classification", "strict": true, "schema": prompt.schema},
        });
    }
    body
}

/// The text of the first choice of a response.
pub fn text_of(response: &Value) -> Result<String, String> {
    response["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| match response["error"]["message"].as_str() {
            Some(message) => format!("the provider answered with an error: {message}"),
            None => "the provider's response has no message".to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt() -> Prompt {
        Prompt {
            system: "s".to_owned(),
            user: "u".to_owned(),
            schema: json!({"type": "object"}),
        }
    }

    #[test]
    fn the_request_asks_for_the_schema_only_when_it_is_structured() {
        let with = request("m", &prompt(), true);
        assert_eq!(with["response_format"]["type"], "json_schema");
        assert_eq!(with["response_format"]["json_schema"]["strict"], true);
        assert_eq!(with["messages"][0]["role"], "system");
        assert!(
            request("m", &prompt(), false)
                .get("response_format")
                .is_none()
        );
    }

    #[test]
    fn the_text_is_the_first_choice_and_an_error_body_is_reported() {
        let ok = json!({"choices": [{"message": {"content": "hi"}}]});
        assert_eq!(text_of(&ok).unwrap(), "hi");
        let err = json!({"error": {"message": "rate limited"}});
        assert!(text_of(&err).unwrap_err().contains("rate limited"));
    }
}
