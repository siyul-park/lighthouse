//! The `typesafe` spec: TypeSafe's System One API. The request body is the
//! one Lighthouse already has, so it is forwarded as it is to
//! `POST {baseUrl}/v1/systemone` (https://docs.typesafe.ai/api), and the
//! answer is read as it comes.

use lighthouse_protocol::{ClassifyParams, ClassifyResult};
use serde_json::{Value, json};

use crate::{http, serve::Endpoint};

/// Asks System One the questions of `params`.
pub fn classify(endpoint: &Endpoint, params: &ClassifyParams) -> Result<ClassifyResult, String> {
    let key = http::key(endpoint.key_env.as_deref())?;
    let url = format!("{}/v1/systemone", endpoint.base_url.trim_end_matches('/'));
    let response =
        http::post(&url, key.as_deref(), &body(endpoint, params)).map_err(|e| e.to_string())?;
    serde_json::from_value(response)
        .map_err(|e| format!("the answer does not fit System One's: {e}"))
}

/// The body of the request: `{state, model, questions}`.
pub fn body(endpoint: &Endpoint, params: &ClassifyParams) -> Value {
    json!({
        "state": params.state,
        "model": endpoint.model,
        "questions": params.questions,
    })
}
