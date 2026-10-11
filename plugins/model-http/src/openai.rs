//! The `openai` spec: Chat Completions. The answer's shape is asked for with
//! `response_format` `json_schema` (OpenAI and OpenRouter document it:
//! https://openrouter.ai/docs/features/structured-outputs). A provider that
//! does not support it refuses the request; the request is then sent again
//! without it, and the text is parsed as strict JSON.

use std::{env, time::Duration};

use serde_json::{Value, json};

use crate::serve::{Endpoint, Prompt, Transport};

/// How long a request may take.
const TIMEOUT: Duration = Duration::from_secs(120);
/// How many times a busy service is asked again.
const RETRIES: usize = 3;

/// Chat Completions over HTTP.
pub struct Http;

impl Http {
    /// Posts, and tries again after a pause when the service is busy (429,
    /// 503) or overloaded (529).
    fn retrying(&self, endpoint: &Endpoint, body: &Value) -> Result<Value, ureq::Error> {
        let mut pause = Duration::from_secs(1);
        for _ in 0..RETRIES {
            match self.post(endpoint, body) {
                Err(ureq::Error::StatusCode(429 | 503 | 529)) => {
                    std::thread::sleep(pause);
                    pause *= 2;
                }
                other => return other,
            }
        }
        self.post(endpoint, body)
    }

    fn post(&self, endpoint: &Endpoint, body: &Value) -> Result<Value, ureq::Error> {
        let url = format!(
            "{}/chat/completions",
            endpoint.base_url.trim_end_matches('/')
        );
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(true)
            .build()
            .into();
        let mut call = agent.post(&url);
        if let Some(key) = endpoint
            .key_env
            .as_deref()
            .and_then(|name| env::var(name).ok())
        {
            call = call.header("Authorization", &format!("Bearer {key}"));
        }
        call.send_json(body)?.body_mut().read_json()
    }
}

impl Transport for Http {
    fn complete(&self, endpoint: &Endpoint, prompt: &Prompt) -> Result<String, String> {
        if let Some(name) = &endpoint.key_env
            && env::var(name).is_err()
        {
            return Err(format!("${name} is not set"));
        }
        let structured = request(&endpoint.model, prompt, true);
        let response = match self.retrying(endpoint, &structured) {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(400 | 404 | 422)) => self
                .retrying(endpoint, &request(&endpoint.model, prompt, false))
                .map_err(|e| e.to_string())?,
            Err(e) => return Err(e.to_string()),
        };
        text_of(&response)
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
