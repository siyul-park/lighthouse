//! TypeSafe's API (https://docs.typesafe.ai/api). One function per operation
//! used. The docs title the endpoint "evaluation" and give no operationId, so
//! the path names it: `systemone`. The bodies are the protocol's
//! [`SystemOneRequest`] and [`SystemOneResponse`], System One's own.

use lighthouse_protocol::{ErrorObject, SYSTEM_ONE, SystemOneRequest, SystemOneResponse};

use crate::{endpoint::Endpoint, http::Client};

/// [`SYSTEM_ONE`]: the questions put to the model, answered as probabilities.
pub fn systemone(
    client: &dyn Client,
    endpoint: &Endpoint,
    request: &SystemOneRequest,
) -> Result<SystemOneResponse, ErrorObject> {
    let mut api_request = request.clone();
    api_request.options.clear();
    let body = serde_json::to_value(api_request).map_err(|e| e.to_string())?;
    let response = client.post(
        &endpoint.url(SYSTEM_ONE.path),
        &endpoint.headers()?,
        &body,
        endpoint.timeout,
    )?;
    serde_json::from_value(response)
        .map_err(|e| format!("the response does not fit System One's: {e}").into())
}

#[cfg(test)]
mod tests;
