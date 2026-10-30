use lighthouse_protocol::{Criteria, Criterion, Instructions, State};
use serde_json::json;

use super::*;
use crate::recording::Recording;

fn question() -> Question {
    let side = || Criterion {
        meaning: "m".to_owned(),
        examples: Vec::new(),
    };
    Question {
        kind: "noul".to_owned(),
        instructions: Instructions {
            requirement: "r".to_owned(),
            prompt: "r".to_owned(),
        },
        criteria: Criteria {
            fail: side(),
            pass: side(),
        },
    }
}

fn request(ids: &[&str]) -> SystemOneRequest {
    SystemOneRequest {
        state: State {
            language: "go".to_owned(),
            file: "a.go".to_owned(),
            text: "func f() {}".to_owned(),
            facts: Value::Null,
            neighbors: Vec::new(),
        },
        model: "qwen/qwen3-coder:free".to_owned(),
        questions: ids
            .iter()
            .map(|id| ((*id).to_owned(), question()))
            .collect(),
        options: Default::default(),
    }
}

/// A reply whose content logprobs are `positions`, each a sampled token
/// and its most likely tokens.
fn logprobs_reply(positions: &[(&str, &[(&str, f32)])]) -> Value {
    let content: Vec<Value> = positions
        .iter()
        .map(|(token, top)| {
            json!({
                "token": token,
                "logprob": 0.0,
                "bytes": null,
                "top_logprobs": top
                    .iter()
                    .map(|(t, p)| json!({"token": t, "logprob": p, "bytes": null}))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({"id": "c", "object": "chat.completion", "created": 1, "model": "m", "choices": [{
        "index": 0,
        "message": {"role": "assistant", "content": "true"},
        "logprobs": {"content": content, "refusal": null},
    }]})
}

fn endpoint(extra: Option<Value>) -> Endpoint {
    let mut endpoint = Endpoint::at("openai", "https://x");
    if let Some(Value::Object(extra)) = extra {
        endpoint.body = extra;
    }
    endpoint
}

/// P(fail) read from one reply.
fn read(reply: Value) -> Result<Option<f64>, ErrorObject> {
    let client = Recording::replying(vec![reply]);
    let result = systemone(&client, &endpoint(None), &request(&["d/x"]))?;
    Ok(result.answers.get("d/x").map(|a| a.noul))
}

fn p_of(is_true: f32, is_false: f32) -> f64 {
    let (is_true, is_false) = (f64::from(is_true).exp(), f64::from(is_false).exp());
    is_true / (is_true + is_false)
}

#[test]
fn each_question_is_one_request_and_its_answer_is_read_from_the_labels() {
    let client = Recording::replying(vec![
        logprobs_reply(&[("true", &[("true", -0.1), ("false", -2.4)])]),
        logprobs_reply(&[("false", &[("false", -0.1), ("true", -2.4)])]),
    ]);
    let endpoint = Endpoint::at("openai", "https://openrouter.ai/api/v1/");
    let result = systemone(&client, &endpoint, &request(&["d/x", "d/y"])).unwrap();
    assert!((result.answers["d/x"].noul - p_of(-0.1, -2.4)).abs() < 1e-9);
    assert!((result.answers["d/y"].noul - p_of(-2.4, -0.1)).abs() < 1e-9);
    assert_eq!(
        result.model,
        format!("qwen/qwen3-coder:free@{PROMPT_VERSION}")
    );
    let asked = client.asked();
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[0].0, "https://openrouter.ai/api/v1/chat/completions");
}

#[test]
fn the_request_has_no_schema_and_one_token() {
    let reply = logprobs_reply(&[("true", &[("true", -0.1), ("False", -2.4)])]);
    let client = Recording::replying(vec![reply]);
    systemone(&client, &endpoint(None), &request(&["d/x"])).unwrap();
    let body = &client.asked()[0].1;
    assert_eq!(body["logprobs"], true);
    assert_eq!(body["top_logprobs"], 20);
    assert_eq!(body["max_tokens"], 1);
    assert_eq!(body["temperature"], 0.0);
    assert!(body.get("response_format").is_none());
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("Answer with one word"));
    assert!(!system.contains("JSON"));
}

#[test]
fn body_overrides_the_logprobs_request_including_max_tokens() {
    let reply = logprobs_reply(&[("true", &[("true", -0.1)])]);
    let client = Recording::replying(vec![reply]);
    let endpoint = endpoint(Some(json!({"max_tokens": 1024, "temperature": 0.5})));
    systemone(&client, &endpoint, &request(&["d/x"])).unwrap();
    let body = &client.asked()[0].1;
    assert_eq!(body["max_tokens"], 1024);
    assert_eq!(body["temperature"], 0.5);
    assert_eq!(body["logprobs"], true);
}

#[test]
fn p_fail_renormalizes_the_probabilities_of_true_and_false() {
    let reply = logprobs_reply(&[("true", &[("true", -0.1), ("False", -2.4), ("maybe", -3.0)])]);
    let p = read(reply).unwrap().unwrap();
    assert!((p - p_of(-0.1, -2.4)).abs() < 1e-9);
}

#[test]
fn labels_are_read_through_whitespace_case_and_punctuation() {
    let reply = logprobs_reply(&[(
        " true",
        &[
            (" true", -0.5),
            ("True.", -1.0),
            ("FALSE", -0.7),
            (" false!", -2.0),
        ],
    )]);
    let p = read(reply).unwrap().unwrap();
    let (is_true, is_false) = (
        (-0.5f64).exp() + (-1.0f64).exp(),
        (-0.7f64).exp() + (-2.0f64).exp(),
    );
    assert!((p - is_true / (is_true + is_false)).abs() < 1e-6);
}

#[test]
fn the_answer_follows_a_closing_reasoning_tag() {
    let reply = logprobs_reply(&[
        ("</think>", &[("</think>", 0.0)]),
        ("true", &[("true", -0.2), ("false", -1.8)]),
    ]);
    let p = read(reply).unwrap().unwrap();
    assert!((p - p_of(-0.2, -1.8)).abs() < 1e-9);
}

#[test]
fn blank_tokens_before_the_answer_are_skipped() {
    let reply = logprobs_reply(&[
        ("\n\n", &[("\n\n", 0.0)]),
        ("false", &[("false", -0.1), ("true", -3.0)]),
    ]);
    let p = read(reply).unwrap().unwrap();
    assert!((p - p_of(-3.0, -0.1)).abs() < 1e-9);
}

#[test]
fn neither_label_is_no_answer() {
    let reply = logprobs_reply(&[("maybe", &[("maybe", -0.1), ("perhaps", -2.0)])]);
    assert_eq!(read(reply).unwrap(), None);
    // `yes` and `no` are not the labels.
    let reply = logprobs_reply(&[("yes", &[("yes", -0.1), ("no", -2.0)])]);
    assert_eq!(read(reply).unwrap(), None);
}

#[test]
fn a_response_without_logprobs_fails_saying_the_model_cannot_classify() {
    for reply in [
        json!({"id": "c", "object": "chat.completion", "created": 1, "model": "m", "choices": [
            {"index": 0, "message": {"role": "assistant", "content": "{\"noul\": 0.97}"}}
        ]}),
        json!({"id": "c", "object": "chat.completion", "created": 1, "model": "m", "choices": []}),
    ] {
        let failed = read(reply).unwrap_err();
        assert!(
            failed.message.contains("returned no logprobs"),
            "{}",
            failed.message
        );
        assert!(failed.message.contains("cannot serve classification"));
    }
}
