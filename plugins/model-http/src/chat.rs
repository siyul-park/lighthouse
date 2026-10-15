//! The one adaptation: a System One request answered through Chat
//! Completions. Each question is rendered as a prompt and answered in one of
//! two ways, by the provider's `logprobs` option.
//!
//! Verbalized (the default): the prompt asks for the probability of yes, and
//! the shape of the answer is asked for with `response_format` `json_schema`
//! (OpenAI and OpenRouter document it:
//! https://openrouter.ai/docs/features/structured-outputs). A provider that
//! does not support it refuses the request, and the call fails with the error
//! body it answered; there is no second request without it.
//!
//! Logprobs: the prompt asks for `true` or `false`, the names of its criteria,
//! and P(fail) is read from the probabilities of those two tokens at the answer
//! position, renormalized over the two labels (OpenAI Cookbook, "Using logprobs
//! for classification"; Zhao et al., ICML 2021). A model whose reasoning cannot
//! be turned off writes it first, so the answer position is the first sampled
//! token that is neither blank nor a closing reasoning tag.

use std::collections::BTreeMap;

use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestUserMessage, ChatCompletionTokenLogprob, CreateChatCompletionRequest,
    CreateChatCompletionResponse, ResponseFormat, ResponseFormatJsonSchema,
};
use lighthouse_protocol::{
    Answer, HandlerError, Question, SystemOneRequest, SystemOneResponse, json_in, noul_of,
    response_schema, subject_text, system_label_text, system_text,
};
use serde_json::{Map, Value};

use crate::{endpoint::Endpoint, http::Client, openai::create_chat_completion};

/// Changes with the prompt, so that cached answers of another prompt are not
/// used.
pub const PROMPT_VERSION: &str = "2";

/// The most likely tokens asked for at each position (the maximum of OpenAI
/// and OpenRouter).
const TOP_LOGPROBS: u8 = 20;

/// Answers the questions of `request`, one chat completion each.
pub fn systemone(
    client: &dyn Client,
    endpoint: &Endpoint,
    request: &SystemOneRequest,
) -> Result<SystemOneResponse, HandlerError> {
    let user = subject_text(&request.state);
    let mut answers = BTreeMap::new();
    for (id, question) in &request.questions {
        let (asked, fixed) = completion(&request.model, id, question, &user, endpoint.logprobs);
        let response = create_chat_completion(client, endpoint, &asked, fixed)?;
        let noul = if endpoint.logprobs {
            label_noul(&response, &request.model)?
        } else {
            verbalized_noul(&response, id)?
        };
        if let Some(noul) = noul {
            answers.insert(
                id.clone(),
                Answer {
                    kind: "noul".to_owned(),
                    noul,
                },
            );
        }
    }
    let mode = if endpoint.logprobs { "-logprobs" } else { "" };
    Ok(SystemOneResponse {
        model: format!("{}@{PROMPT_VERSION}{mode}", request.model),
        answers,
        usage: Value::Null,
    })
}

/// The probability of yes a model wrote as JSON.
fn verbalized_noul(
    response: &CreateChatCompletionResponse,
    id: &str,
) -> Result<Option<f64>, HandlerError> {
    let text = response
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_deref())
        .ok_or("the provider's response has no message")?;
    Ok(json_in(text).and_then(|value| noul_of(&value, id)))
}

/// P(fail) from the probabilities of `true` and `false` at the answer position;
/// `None` when neither is among the most likely tokens.
fn label_noul(
    response: &CreateChatCompletionResponse,
    model: &str,
) -> Result<Option<f64>, HandlerError> {
    let content = response
        .choices
        .first()
        .and_then(|choice| choice.logprobs.as_ref())
        .and_then(|logprobs| logprobs.content.as_deref())
        .ok_or_else(|| {
            format!("model {model} returned no logprobs: the provider ignored the parameter")
        })?;
    let Some(answer) = content.iter().find(|at| is_answer(at)) else {
        return Ok(None);
    };
    let mass = |label: &str| -> f64 {
        answer
            .top_logprobs
            .iter()
            .filter(|top| normalized(&top.token) == label)
            .map(|top| f64::from(top.logprob).exp())
            .sum()
    };
    let (is_true, is_false) = (mass("true"), mass("false"));
    Ok((is_true + is_false > 0.0).then(|| is_true / (is_true + is_false)))
}

/// Whether a sampled token is where the answer is: not blank, and not the
/// closing tag of reasoning (`</think>`).
fn is_answer(at: &ChatCompletionTokenLogprob) -> bool {
    let token = at.token.trim();
    !token.is_empty() && !(token.starts_with("</") && token.ends_with('>'))
}

/// A token without whitespace and surrounding punctuation, in lower case.
fn normalized(token: &str) -> String {
    token
        .trim()
        .trim_matches(|c: char| c.is_ascii_punctuation())
        .to_lowercase()
}

/// The chat completion that asks one question, and the fields of it that the
/// request type cannot carry (`max_tokens` is deprecated there, and the
/// providers still read it).
fn completion(
    model: &str,
    id: &str,
    question: &Question,
    user: &str,
    logprobs: bool,
) -> (CreateChatCompletionRequest, Map<String, Value>) {
    let system = if logprobs {
        system_label_text(id, question)
    } else {
        system_text(id, question)
    };
    let mut request = CreateChatCompletionRequest {
        model: model.to_owned(),
        temperature: Some(0.0),
        messages: vec![
            ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage::from(
                system.as_str(),
            )),
            ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage::from(user)),
        ],
        ..Default::default()
    };
    let mut fixed = Map::new();
    if logprobs {
        request.logprobs = Some(true);
        request.top_logprobs = Some(TOP_LOGPROBS);
        fixed.insert("max_tokens".to_owned(), Value::from(1));
    } else {
        request.response_format = Some(ResponseFormat::JsonSchema {
            json_schema: ResponseFormatJsonSchema {
                description: None,
                name: "classification".to_owned(),
                schema: response_schema(),
                strict: Some(true),
            },
        });
    }
    (request, fixed)
}

#[cfg(test)]
mod tests {
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

    fn replying(texts: &[&str]) -> Recording {
        Recording::replying(
            texts
                .iter()
                .map(|t| json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": t}}], "id": "c", "object": "chat.completion", "created": 1, "model": "m"}))
                .collect(),
        )
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

    fn labelled(endpoint_extra: Option<Value>) -> Endpoint {
        let mut endpoint = Endpoint::at("openai", "https://x");
        endpoint.logprobs = true;
        if let Some(Value::Object(extra)) = endpoint_extra {
            endpoint.extra_body = extra;
        }
        endpoint
    }

    /// P(fail) the logprobs mode reads from one reply.
    fn read(reply: Value) -> Result<Option<f64>, HandlerError> {
        let client = Recording::replying(vec![reply]);
        let result = systemone(&client, &labelled(None), &request(&["d/x"]))?;
        Ok(result.answers.get("d/x").map(|a| a.noul))
    }

    fn p_of(is_true: f32, is_false: f32) -> f64 {
        let (is_true, is_false) = (f64::from(is_true).exp(), f64::from(is_false).exp());
        is_true / (is_true + is_false)
    }

    #[test]
    fn the_completion_asks_for_the_schema_with_the_prompt() {
        let (asked, fixed) = completion("m", "d/x", &question(), "u", false);
        let body = serde_json::to_value(asked).unwrap();
        assert_eq!(body["model"], "m");
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "u");
        assert!(body.get("logprobs").is_none() && fixed.is_empty());
    }

    #[test]
    fn each_question_is_one_request_and_a_fenced_probability_is_its_answer() {
        let client = replying(&["```json\n{\"noul\": 0.93}\n```", "{\"noul\": 0.1}"]);
        let endpoint = Endpoint::at("openai", "https://openrouter.ai/api/v1/");
        let result = systemone(&client, &endpoint, &request(&["d/x", "d/y"])).unwrap();
        assert_eq!(result.answers["d/x"].noul, 0.93);
        assert_eq!(result.answers["d/y"].noul, 0.1);
        assert!(result.model.starts_with("qwen/qwen3-coder:free@"));
        assert!(!result.model.ends_with("-logprobs"));
        let asked = client.asked();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[0].0, "https://openrouter.ai/api/v1/chat/completions");
    }

    #[test]
    fn text_that_is_not_a_probability_is_no_answer() {
        for text in ["I think it is bad", "{\"noul\": 7}"] {
            let endpoint = Endpoint::at("openai", "https://x");
            let result = systemone(&replying(&[text]), &endpoint, &request(&["d/x"])).unwrap();
            assert!(result.answers.is_empty(), "{text}");
        }
    }

    #[test]
    fn a_response_without_a_message_fails() {
        let client = Recording::replying(vec![
            json!({"id": "c", "object": "chat.completion", "created": 1, "model": "m", "choices": []}),
        ]);
        let endpoint = Endpoint::at("openai", "https://x");
        assert!(systemone(&client, &endpoint, &request(&["d/x"])).is_err());
    }

    #[test]
    fn the_logprobs_request_has_no_schema_and_one_token() {
        let reply = logprobs_reply(&[("true", &[("true", -0.1), ("False", -2.4)])]);
        let client = Recording::replying(vec![reply]);
        systemone(&client, &labelled(None), &request(&["d/x"])).unwrap();
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
    fn extra_body_overrides_the_logprobs_request_including_max_tokens() {
        let reply = logprobs_reply(&[("true", &[("true", -0.1)])]);
        let client = Recording::replying(vec![reply]);
        let endpoint = labelled(Some(json!({"max_tokens": 1024, "temperature": 0.5})));
        systemone(&client, &endpoint, &request(&["d/x"])).unwrap();
        let body = &client.asked()[0].1;
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["logprobs"], true);
    }

    #[test]
    fn p_fail_renormalizes_the_probabilities_of_true_and_false() {
        let reply =
            logprobs_reply(&[("true", &[("true", -0.1), ("False", -2.4), ("maybe", -3.0)])]);
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
    fn a_response_without_logprobs_fails_naming_the_model() {
        let failed = read(
            json!({"id": "c", "object": "chat.completion", "created": 1, "model": "m", "choices": [
                {"index": 0, "message": {"role": "assistant", "content": "true"}}
            ]}),
        )
        .unwrap_err();
        assert!(failed.message.contains("qwen/qwen3-coder:free"));
    }

    #[test]
    fn the_logprobs_answers_have_a_model_id_of_their_own() {
        let reply = logprobs_reply(&[("true", &[("true", -0.1)])]);
        let client = Recording::replying(vec![reply]);
        let result = systemone(&client, &labelled(None), &request(&["d/x"])).unwrap();
        assert_eq!(
            result.model,
            format!("qwen/qwen3-coder:free@{PROMPT_VERSION}-logprobs")
        );
    }
}
