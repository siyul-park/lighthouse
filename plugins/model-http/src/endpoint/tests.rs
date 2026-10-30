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
