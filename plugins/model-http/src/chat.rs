//! The one adaptation: a System One request answered through Chat
//! Completions. Each question is rendered as a prompt that asks for the
//! probability of yes, and the shape of the answer is asked for with
//! `response_format` `json_schema` (OpenAI and OpenRouter document it:
//! https://openrouter.ai/docs/features/structured-outputs). A provider that
//! does not support it refuses the request, and the call fails with the error
//! body it answered; there is no second request without it.

use std::collections::BTreeMap;

use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestUserMessage, CreateChatCompletionRequest, ResponseFormat,
    ResponseFormatJsonSchema,
};
use lighthouse_protocol::{
    Answer, HandlerError, Question, SystemOneRequest, SystemOneResponse, json_in, noul_of,
    response_schema, subject_text, system_text,
};
use serde_json::Value;

use crate::{endpoint::Endpoint, http::Client, openai::create_chat_completion};

/// Changes with the prompt, so that cached answers of another prompt are not
/// used.
pub const PROMPT_VERSION: &str = "2";

/// Answers the questions of `request`, one chat completion each.
pub fn systemone(
    client: &dyn Client,
    endpoint: &Endpoint,
    request: &SystemOneRequest,
) -> Result<SystemOneResponse, HandlerError> {
    let user = subject_text(&request.state);
    let mut answers = BTreeMap::new();
    for (id, question) in &request.questions {
        let asked = completion(&request.model, id, question, &user);
        let response = create_chat_completion(client, endpoint, &asked)?;
        let text = response
            .choices
            .first()
            .and_then(|choice| choice.message.content.as_deref())
            .ok_or("the provider's response has no message")?;
        if let Some(noul) = json_in(text).and_then(|value| noul_of(&value, id)) {
            answers.insert(
                id.clone(),
                Answer {
                    kind: "noul".to_owned(),
                    noul,
                },
            );
        }
    }
    Ok(SystemOneResponse {
        model: format!("{}@{PROMPT_VERSION}", request.model),
        answers,
        usage: Value::Null,
    })
}

/// The chat completion that asks one question.
fn completion(
    model: &str,
    id: &str,
    question: &Question,
    user: &str,
) -> CreateChatCompletionRequest {
    CreateChatCompletionRequest {
        model: model.to_owned(),
        temperature: Some(0.0),
        messages: vec![
            ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage::from(
                system_text(id, question).as_str(),
            )),
            ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage::from(user)),
        ],
        response_format: Some(ResponseFormat::JsonSchema {
            json_schema: ResponseFormatJsonSchema {
                description: None,
                name: "classification".to_owned(),
                schema: response_schema(),
                strict: Some(true),
            },
        }),
        ..Default::default()
    }
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

    #[test]
    fn the_completion_asks_for_the_schema_with_the_prompt() {
        let body = serde_json::to_value(completion("m", "d/x", &question(), "u")).unwrap();
        assert_eq!(body["model"], "m");
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "u");
    }

    #[test]
    fn each_question_is_one_request_and_a_fenced_probability_is_its_answer() {
        let client = replying(&["```json\n{\"noul\": 0.93}\n```", "{\"noul\": 0.1}"]);
        let endpoint = Endpoint::at("openai", "https://openrouter.ai/api/v1/");
        let result = systemone(&client, &endpoint, &request(&["d/x", "d/y"])).unwrap();
        assert_eq!(result.answers["d/x"].noul, 0.93);
        assert_eq!(result.answers["d/y"].noul, 0.1);
        assert!(result.model.starts_with("qwen/qwen3-coder:free@"));
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
}
