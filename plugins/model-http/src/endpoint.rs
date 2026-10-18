//! How a provider is reached, from the `options` of its declaration.

use std::time::Duration;

use lighthouse_protocol::ErrorObject;
use lighthouse_resource::Duration as Span;
use serde_json::{Map, Value};

/// How long a request may take when the provider does not say.
const TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, PartialEq)]
pub struct Endpoint {
    /// The protocols (published APIs) the provider declares.
    pub protocols: Vec<String>,
    pub base_url: String,
    pub key_env: Option<String>,
    /// Merged shallowly into every request body; its keys override.
    pub extra_body: Map<String, Value>,
    /// How long one request may take.
    pub timeout: Duration,
}

impl Endpoint {
    /// Reads the options of a provider: `protocol` (one name or a list),
    /// `baseUrl`, `keyEnv`, `extraBody` and `timeout` (a duration such as
    /// `10m`).
    pub fn of(options: &Map<String, Value>) -> Result<Self, ErrorObject> {
        let text = |key: &str| options.get(key).and_then(Value::as_str).map(str::to_owned);
        let protocols = match options.get("protocol") {
            Some(Value::String(name)) => vec![name.clone()],
            Some(Value::Array(list)) => list
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => return Err("the provider declares no `protocol`".into()),
        };
        let timeout = match text("timeout") {
            Some(span) => span
                .parse::<Span>()
                .map_err(|e| format!("`timeout`: {e}"))?
                .get(),
            None => TIMEOUT,
        };
        Ok(Self {
            protocols,
            base_url: text("baseUrl").ok_or("the provider declares no `baseUrl`")?,
            key_env: text("keyEnv"),
            extra_body: options
                .get("extraBody")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            timeout,
        })
    }

    /// Whether the provider declares `protocol`.
    pub fn speaks(&self, protocol: &str) -> bool {
        self.protocols.iter().any(|p| p == protocol)
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
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    /// A endpoint for tests.
    #[cfg(test)]
    pub fn at(protocol: &str, base_url: &str) -> Self {
        Self {
            protocols: vec![protocol.to_owned()],
            base_url: base_url.to_owned(),
            key_env: None,
            extra_body: Map::new(),
            timeout: TIMEOUT,
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
    fn the_endpoint_reads_extra_body() {
        let endpoint = Endpoint::of(&options(json!({
            "protocol": "openai", "baseUrl": "https://x",
            "extraBody": {"max_tokens": 1024}
        })))
        .unwrap();
        assert_eq!(endpoint.extra_body["max_tokens"], 1024);
    }

    #[test]
    fn the_endpoint_reads_the_timeout_and_defaults_to_two_minutes() {
        let read = |timeout: Option<&str>| {
            let mut declared = json!({"protocol": "openai", "baseUrl": "https://x"});
            if let Some(timeout) = timeout {
                declared["timeout"] = json!(timeout);
            }
            Endpoint::of(&options(declared))
        };
        assert_eq!(read(None).unwrap().timeout, Duration::from_secs(120));
        assert_eq!(read(Some("10m")).unwrap().timeout, Duration::from_secs(600));
        assert_eq!(read(Some("90s")).unwrap().timeout, Duration::from_secs(90));
        assert!(read(Some("soon")).is_err());
    }

    #[test]
    fn the_endpoint_needs_a_protocol_and_a_base_url() {
        assert!(Endpoint::of(&Map::new()).is_err());
        assert!(Endpoint::of(&options(json!({"protocol": "openai"}))).is_err());
        let endpoint = Endpoint::of(&options(
            json!({"protocol": ["openai", "typesafe"], "baseUrl": "https://x/v1/", "keyEnv": "K"}),
        ))
        .unwrap();
        assert!(endpoint.speaks("typesafe") && !endpoint.speaks("anthropic"));
        assert_eq!(endpoint.key_env.as_deref(), Some("K"));
        assert!(endpoint.extra_body.is_empty());
        assert_eq!(
            endpoint.url("chat/completions"),
            "https://x/v1/chat/completions"
        );
    }
}
