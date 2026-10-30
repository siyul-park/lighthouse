//! The one adaptation: a System One request answered through Chat
//! Completions. This is the only use of Chat Completions here, and it is
//! System One's: the logprobs of a chat model are the wire of `classification`.
//! A chat model never generates text for Lighthouse (no fix, no check, no
//! decision); that is an external agent's work (`edit`). Each question is rendered as a prompt asking for `true` or
//! `false`, the names of its criteria, and P(fail) is read from the
//! probabilities of those two tokens at the answer position, renormalized over
//! the two labels (OpenAI Cookbook, "Using logprobs for classification"; Zhao
//! et al., ICML 2021). A model whose reasoning cannot be turned off writes it
//! first, so the answer position is the first sampled token that is neither
//! blank nor a closing reasoning tag.
//!
//! The probability is the model's own distribution: a number the model writes
//! is never read as one, so a model that returns no logprobs cannot serve
//! classification.

use std::collections::BTreeMap;

use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestUserMessage, ChatCompletionTokenLogprob, CreateChatCompletionRequest,
    CreateChatCompletionResponse,
};
use lighthouse_protocol::{
    Answer, ErrorObject, Question, SystemOneRequest, SystemOneResponse, prompt,
};
use serde_json::{Map, Value};

use crate::{endpoint::Endpoint, http::Client, openai::create_chat_completion};

/// Changes with the prompt, so that cached answers of another prompt are not
/// used.
pub const PROMPT_VERSION: &str = "3";

/// The most likely tokens asked for at each position (the maximum of OpenAI
/// and OpenRouter).
const TOP_LOGPROBS: u8 = 20;

/// Answers the questions of `request`, one chat completion each.
pub fn systemone(
    client: &dyn Client,
    endpoint: &Endpoint,
    request: &SystemOneRequest,
) -> Result<SystemOneResponse, ErrorObject> {
    let user = prompt::subject(&request.state);
    let mut answers = BTreeMap::new();
    for (id, question) in &request.questions {
        let (asked, fixed) = completion(&request.model, id, question, &user);
        let response = create_chat_completion(client, endpoint, &asked, fixed)?;
        if let Some(noul) = label_noul(&response)? {
            answers.insert(id.clone(), Answer::new(noul));
        }
    }
    Ok(SystemOneResponse {
        model: format!("{}@{PROMPT_VERSION}", request.model),
        answers,
        usage: Value::Null,
    })
}

/// P(fail) from the probabilities of `true` and `false` at the answer position;
/// `None` when neither is among the most likely tokens.
fn label_noul(response: &CreateChatCompletionResponse) -> Result<Option<f64>, ErrorObject> {
    let content = response
        .choices
        .first()
        .and_then(|choice| choice.logprobs.as_ref())
        .and_then(|logprobs| logprobs.content.as_deref())
        .ok_or("the model returned no logprobs; it cannot serve classification")?;
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
) -> (CreateChatCompletionRequest, Map<String, Value>) {
    let system = prompt::system(id, question);
    let request = CreateChatCompletionRequest {
        model: model.to_owned(),
        temperature: Some(0.0),
        messages: vec![
            ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage::from(
                system.as_str(),
            )),
            ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage::from(user)),
        ],
        logprobs: Some(true),
        top_logprobs: Some(TOP_LOGPROBS),
        ..Default::default()
    };
    let fixed = Map::from_iter([("max_tokens".to_owned(), Value::from(1))]);
    (request, fixed)
}

#[cfg(test)]
mod tests;
