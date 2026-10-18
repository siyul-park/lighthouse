//! How a provider is reached, from the `options` of its declaration.

use std::{collections::BTreeMap, time::Duration};

use lighthouse_process::expand_value;
use lighthouse_protocol::ErrorObject;
use lighthouse_resource::Duration as Span;
use serde_json::{Map, Value};

/// How long a request may take when the provider does not say.
const TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, PartialEq)]
pub struct Endpoint {
    /// The protocols (published APIs) the provider declares.
    pub protocols: Vec<String>,
    pub url: String,
    /// The headers of every request; a value may hold `${NAME}`, which
    /// [`Endpoint::headers`] expands when a request is made.
    declared: BTreeMap<String, String>,
    /// Merged shallowly into every request body; its keys override.
    pub body: Map<String, Value>,
    /// How long one request may take.
    pub timeout: Duration,
}

impl Endpoint {
    /// Reads the options of a provider: `protocol` (one name or a list),
    /// `url`, `headers`, `body` and `timeout` (a duration such as `10m`).
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
        let declared = options
            .get("headers")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
            .collect();
        Ok(Self {
            protocols,
            url: text("url").ok_or("the provider declares no `url`")?,
            declared,
            body: options
                .get("body")
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

    /// The headers of a request, with each `${NAME}` replaced by the variable
    /// it names. A variable that is not set fails the request, naming it.
    pub fn headers(&self) -> Result<Vec<(String, String)>, ErrorObject> {
        self.declared
            .iter()
            .map(|(name, value)| {
                expand_value(value)
                    .map(|expanded| (name.clone(), expanded))
                    .map_err(|why| format!("`headers.{name}`: {why}").into())
            })
            .collect()
    }

    /// `path` under the url.
    pub fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    /// A endpoint for tests.
    #[cfg(test)]
    pub fn at(protocol: &str, url: &str) -> Self {
        Self {
            protocols: vec![protocol.to_owned()],
            url: url.to_owned(),
            declared: BTreeMap::new(),
            body: Map::new(),
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
    fn the_endpoint_reads_the_body() {
        let endpoint = Endpoint::of(&options(json!({
            "protocol": "openai", "url": "https://x",
            "body": {"max_tokens": 1024}
        })))
        .unwrap();
        assert_eq!(endpoint.body["max_tokens"], 1024);
    }

    #[test]
    fn the_endpoint_reads_the_timeout_and_defaults_to_two_minutes() {
        let read = |timeout: Option<&str>| {
            let mut declared = json!({"protocol": "openai", "url": "https://x"});
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
    fn the_endpoint_needs_a_protocol_and_a_url() {
        assert!(Endpoint::of(&Map::new()).is_err());
        assert!(Endpoint::of(&options(json!({"protocol": "openai"}))).is_err());
        let endpoint = Endpoint::of(&options(
            json!({"protocol": ["openai", "typesafe"], "url": "https://x/v1/"}),
        ))
        .unwrap();
        assert!(endpoint.speaks("typesafe") && !endpoint.speaks("anthropic"));
        assert!(endpoint.body.is_empty());
        assert_eq!(
            endpoint.url("chat/completions"),
            "https://x/v1/chat/completions"
        );
    }

    #[test]
    fn a_header_expands_the_variable_it_names_and_a_missing_one_is_named() {
        let endpoint = Endpoint::of(&options(json!({
            "protocol": "openai", "url": "https://x",
            "headers": {"Authorization": "Bearer ${PATH}", "x-key": "${LIGHTHOUSE_NOT_SET_FOR_TEST}"}
        })))
        .unwrap();
        let failed = endpoint.headers().unwrap_err();
        assert!(
            failed.message.contains("LIGHTHOUSE_NOT_SET_FOR_TEST"),
            "{failed:?}"
        );
        let only_set = Endpoint::of(&options(json!({
            "protocol": "openai", "url": "https://x",
            "headers": {"Authorization": "Bearer ${PATH}"}
        })))
        .unwrap();
        let headers = only_set.headers().unwrap();
        assert_eq!(headers[0].0, "Authorization");
        assert!(headers[0].1.starts_with("Bearer /"), "{headers:?}");
    }
}
