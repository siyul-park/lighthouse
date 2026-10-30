//! How a provider is reached, from the `options` of its declaration.

use std::{collections::BTreeMap, time::Duration};

use lighthouse_model::Duration as Span;
use lighthouse_process::expand_value;
use lighthouse_protocol::ErrorObject;
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
mod tests;
