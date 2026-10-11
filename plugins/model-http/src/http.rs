//! The HTTP all specs share: a JSON POST with a bearer key (RFC 6750), and a
//! pause before asking again when the service is busy (429, 503) or
//! overloaded (529).

use std::{env, time::Duration};

use serde_json::Value;

/// How long a request may take.
const TIMEOUT: Duration = Duration::from_secs(120);
/// How many times a busy service is asked again.
const RETRIES: usize = 3;

/// The key in the environment variable `name`, `Ok(None)` when the provider
/// needs none.
pub fn key(name: Option<&str>) -> Result<Option<String>, String> {
    match name {
        None => Ok(None),
        Some(name) => env::var(name)
            .map(Some)
            .map_err(|_| format!("${name} is not set")),
    }
}

/// Posts `body` to `url`, again after a pause while the service is busy.
pub fn post(url: &str, key: Option<&str>, body: &Value) -> Result<Value, ureq::Error> {
    let mut pause = Duration::from_secs(1);
    for _ in 0..RETRIES {
        match post_once(url, key, body) {
            Err(ureq::Error::StatusCode(429 | 503 | 529)) => {
                std::thread::sleep(pause);
                pause *= 2;
            }
            other => return other,
        }
    }
    post_once(url, key, body)
}

fn post_once(url: &str, key: Option<&str>, body: &Value) -> Result<Value, ureq::Error> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(true)
        .build()
        .into();
    let mut call = agent.post(url);
    if let Some(key) = key {
        call = call.header("Authorization", &format!("Bearer {key}"));
    }
    call.send_json(body)?.body_mut().read_json()
}
