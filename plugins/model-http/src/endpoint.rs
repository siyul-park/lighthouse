//! How a provider is reached, from the `options` of its declaration.

use lighthouse_protocol::ErrorObject;
use serde_json::{Map, Value};

#[derive(Debug, PartialEq)]
pub struct Endpoint {
    /// The specs (published APIs) the provider declares.
    pub specs: Vec<String>,
    pub base_url: String,
    pub key_env: Option<String>,
    /// Read the answer from the probability of the first token.
    pub logprobs: bool,
    /// Merged shallowly into every request body; its keys override.
    pub extra_body: Map<String, Value>,
}

impl Endpoint {
    /// Reads the options of a provider: `spec` (one name or a list),
    /// `baseUrl`, `keyEnv`, `logprobs` and `extraBody`.
    pub fn of(options: &Map<String, Value>) -> Result<Self, ErrorObject> {
        let text = |key: &str| options.get(key).and_then(Value::as_str).map(str::to_owned);
        let specs = match options.get("spec") {
            Some(Value::String(name)) => vec![name.clone()],
            Some(Value::Array(list)) => list
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => return Err("the provider declares no `spec`".into()),
        };
        Ok(Self {
            specs,
            base_url: text("baseUrl").ok_or("the provider declares no `baseUrl`")?,
            key_env: text("keyEnv"),
            logprobs: options.get("logprobs").and_then(Value::as_bool) == Some(true),
            extra_body: options
                .get("extraBody")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        })
    }

    /// Whether the provider declares `spec`.
    pub fn speaks(&self, spec: &str) -> bool {
        self.specs.iter().any(|s| s == spec)
    }

    /// The key in the environment variable `keyEnv` names; `None` when the
    /// provider needs none.
    pub fn key(&self) -> Result<Option<String>, ErrorObject> {
        match self.key_env.as_deref() {
            None => Ok(None),
            Some(name) => std::env::var(name)
                .map(Some)
                .map_err(|_| format!("${name} is not set").into()),
        }
    }

    /// `path` under the base URL.
    pub fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base_url.trim_end_matches('/'))
    }

    /// A endpoint for tests.
    #[cfg(test)]
    pub fn at(spec: &str, base_url: &str) -> Self {
        Self {
            specs: vec![spec.to_owned()],
            base_url: base_url.to_owned(),
            key_env: None,
            logprobs: false,
            extra_body: Map::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn options(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn the_endpoint_reads_logprobs_and_extra_body() {
        let endpoint = Endpoint::of(&options(json!({
            "spec": "openai", "baseUrl": "https://x", "logprobs": true,
            "extraBody": {"max_tokens": 1024}
        })))
        .unwrap();
        assert!(endpoint.logprobs);
        assert_eq!(endpoint.extra_body["max_tokens"], 1024);
    }

    #[test]
    fn the_endpoint_needs_a_spec_and_a_base_url() {
        assert!(Endpoint::of(&Map::new()).is_err());
        assert!(Endpoint::of(&options(json!({"spec": "openai"}))).is_err());
        let endpoint = Endpoint::of(&options(
            json!({"spec": ["openai", "typesafe"], "baseUrl": "https://x/v1/", "keyEnv": "K"}),
        ))
        .unwrap();
        assert!(endpoint.speaks("typesafe") && !endpoint.speaks("anthropic"));
        assert_eq!(endpoint.key_env.as_deref(), Some("K"));
        assert!(!endpoint.logprobs && endpoint.extra_body.is_empty());
        assert_eq!(
            endpoint.url("chat/completions"),
            "https://x/v1/chat/completions"
        );
    }
}
