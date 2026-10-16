//! The HTTP all specs share. [`Client`] is the one seam for I/O: a JSON POST
//! with a bearer key (RFC 6750), answered with the JSON body or the error. A
//! spec takes a `&dyn Client`; tests swap the client, never a spec. [`Http`] is
//! the client: it pauses before asking again when the service is busy (429,
//! 503) or overloaded (529), for the `Retry-After` the service sent, in
//! seconds, else a growing time. Nothing else is asked again.

use std::time::Duration;

use lighthouse_protocol::HandlerError;
use serde_json::Value;

/// How long a request may take.
const TIMEOUT: Duration = Duration::from_secs(120);
/// How many times a busy service is asked again.
const RETRIES: usize = 3;
/// The longest a service may ask to wait.
const MAX_PAUSE: Duration = Duration::from_secs(60);

/// Something that posts a JSON body and gives the JSON that answers.
pub trait Client: Sync {
    /// Posts `body` to `url`, with `key` as the bearer when there is one. An
    /// error status fails with the error body the service answered, which
    /// travels as the `data` of the JSON-RPC error.
    fn post(&self, url: &str, key: Option<&str>, body: &Value) -> Result<Value, HandlerError>;
}

/// The client over the network: one agent, so connections are reused.
pub struct Http {
    agent: ureq::Agent,
}

impl Http {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        Self { agent }
    }
}

impl Client for Http {
    fn post(&self, url: &str, key: Option<&str>, body: &Value) -> Result<Value, HandlerError> {
        let mut pause = Duration::from_secs(1);
        for _ in 0..RETRIES {
            match post(&self.agent, url, key, body) {
                Err(refused) if matches!(refused.status, Some(429 | 503 | 529)) => {
                    let wait = refused
                        .retry_after
                        .map_or(pause, Duration::from_secs)
                        .min(MAX_PAUSE);
                    std::thread::sleep(wait);
                    pause *= 2;
                }
                other => return other.map_err(|refused| refused.error),
            }
        }
        post(&self.agent, url, key, body).map_err(|refused| refused.error)
    }
}

/// A request that failed: refused with an error status, or not answered.
struct Refused {
    /// The status the service answered with; none when it was not reached.
    status: Option<u16>,
    /// The seconds the service asked to wait.
    retry_after: Option<u64>,
    error: HandlerError,
}

fn unreached(e: ureq::Error) -> Refused {
    Refused {
        status: None,
        retry_after: None,
        error: e.to_string().into(),
    }
}

fn post(agent: &ureq::Agent, url: &str, key: Option<&str>, body: &Value) -> Result<Value, Refused> {
    let mut call = agent.post(url);
    if let Some(key) = key {
        call = call.header("Authorization", &format!("Bearer {key}"));
    }
    let mut response = call.send_json(body).map_err(unreached)?;
    let status = response.status().as_u16();
    if response.status().is_success() {
        return response.body_mut().read_json().map_err(unreached);
    }
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok());
    let text = response.body_mut().read_to_string().unwrap_or_default();
    let message = format!("the service answered {status}");
    let error = if text.trim().is_empty() {
        HandlerError::from(message)
    } else {
        let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
        HandlerError::with_data(message, body)
    };
    Err(Refused {
        status: Some(status),
        retry_after,
        error,
    })
}
