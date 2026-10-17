//! Asking the chat route for text (the fix of a finding): the first choice of
//! a completion. Answers are kept under the model at its version and the
//! digest of the request.

use lighthouse_plugin::{ChatRequest, CreateChatCompletionRequest};

use super::route::{Member, Refusal, Route};
use crate::cache::{Digest, KeyBuilder, digest_of};

/// An answer: the text of the first choice and the model that gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reply {
    pub(crate) text: String,
    /// `<provider>/<model>`.
    pub(crate) model: String,
}

impl Route<'_> {
    /// The answer of the route to the request `build` writes: it gets whether
    /// the model takes a response format, which a plain-text command does not.
    pub(crate) fn ask(
        &self,
        build: &dyn Fn(bool) -> CreateChatCompletionRequest,
    ) -> Result<Reply, Refusal> {
        let key = |member: &Member<'_>, model: &str| answer_key(model, &build(!member.plain));
        let call = |member: &Member<'_>| {
            let request = ChatRequest {
                body: build(!member.plain),
                options: serde_json::Map::new(),
            };
            member
                .client
                .chat(&request)
                .map_err(|e| e.to_string())
                .and_then(|response| {
                    response
                        .choices
                        .into_iter()
                        .next()
                        .and_then(|choice| choice.message.content)
                        .ok_or_else(|| "it answered with no text".to_owned())
                })
        };
        self.through(&key, &call)
            .map(|(text, model)| Reply { text, model })
    }
}

/// The key of an answer: the model at its version, and what it is asked.
fn answer_key(model: &str, body: &CreateChatCompletionRequest) -> Digest {
    let mut key = KeyBuilder::new();
    key.part("chat-answer")
        .part(model)
        .part(digest_of(body).unwrap_or_default());
    key.finish()
}
