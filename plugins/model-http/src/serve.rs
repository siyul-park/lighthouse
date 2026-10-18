//! The plugin as the protocol sees it: the handshake, and each method handed
//! to the protocol that serves it.

use lighthouse_protocol::{
    Capabilities, ErrorObject, Handler, InitializeParams, InitializeResult, Model,
    SystemOneRequest, SystemOneResponse,
};

use crate::{
    chat::{self, PROMPT_VERSION},
    endpoint::Endpoint,
    http::Client,
    typesafe,
};

/// The plugin's id, and the id of the one model it offers.
pub const ID: &str = "model-http";

pub struct Plugin {
    client: Box<dyn Client>,
}

impl Plugin {
    pub fn new(client: Box<dyn Client>) -> Self {
        Self { client }
    }
}

impl Handler for Plugin {
    fn initialize(&mut self, _: InitializeParams) -> Result<InitializeResult, ErrorObject> {
        let model = Model {
            abstains: true,
            cost: 1.0,
            ..Model::new(ID, PROMPT_VERSION, Capabilities::classifier(false))
        };
        Ok(InitializeResult::new(
            ID,
            env!("CARGO_PKG_VERSION"),
            Vec::new(),
            vec![model],
        ))
    }

    fn classify(&mut self, params: SystemOneRequest) -> Result<SystemOneResponse, ErrorObject> {
        if params.model.is_empty() {
            return Err("the request names no model".into());
        }
        let endpoint = Endpoint::of(&params.options)?;
        // The method's own protocol first, then Chat Completions by adaptation.
        let client = self.client.as_ref();
        if endpoint.speaks("typesafe") {
            typesafe::systemone(client, &endpoint, &params)
        } else if endpoint.speaks("openai") {
            chat::systemone(client, &endpoint, &params)
        } else {
            Err(format!(
                "no protocol of {:?} serves classification (served: [\"typesafe\", \"openai\"])",
                endpoint.protocols
            )
            .into())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread::{self, JoinHandle},
    };

    use lighthouse_protocol::{
        Criteria, Criterion, Instructions, Question, State, SystemOneRequest,
    };
    use serde_json::{Value, json};

    use super::*;
    use crate::http::Http;

    fn params(protocol: Value, url: &str) -> SystemOneRequest {
        let side = || Criterion {
            meaning: "m".to_owned(),
            examples: Vec::new(),
        };
        SystemOneRequest {
            state: State {
                language: "go".to_owned(),
                file: "a.go".to_owned(),
                text: "func f() {}".to_owned(),
                facts: Value::Null,
                neighbors: Vec::new(),
            },
            model: "qwen/qwen3-coder:free".to_owned(),
            questions: [(
                "d/x".to_owned(),
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
                },
            )]
            .into(),
            options: json!({ "protocol": protocol, "url": url })
                .as_object()
                .unwrap()
                .clone(),
        }
    }

    /// Serves one canned response on a local port; returns the base URL and
    /// the request it got.
    fn serve_once(status: &'static str, body: &'static str) -> (String, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = stream.read(&mut chunk).unwrap();
                request.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&request).into_owned();
                let Some((head, rest)) = text.split_once("\r\n\r\n") else {
                    continue;
                };
                let length = head
                    .lines()
                    .filter_map(|l| {
                        l.to_lowercase()
                            .strip_prefix("content-length:")
                            .map(str::to_owned)
                    })
                    .find_map(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if rest.len() >= length {
                    break;
                }
            }
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (url, handle)
    }

    #[test]
    fn the_openai_protocol_posts_chat_completions_over_the_network() {
        let (url, request) = serve_once(
            "200 OK",
            r#"{"id":"c","object":"chat.completion","created":1,"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"true"},"logprobs":{"content":[{"token":"true","logprob":-0.05,"bytes":null,"top_logprobs":[{"token":"true","logprob":-0.05,"bytes":null},{"token":"false","logprob":-3.0,"bytes":null}]}],"refusal":null}}]}"#,
        );
        let result = Plugin::new(Box::new(Http::new()))
            .classify(params(json!("openai"), &url))
            .unwrap();
        let request = request.join().unwrap();

        assert!(
            request.starts_with("POST /chat/completions HTTP/1.1"),
            "{request}"
        );
        assert!(request.contains("top_logprobs") && request.contains("qwen/qwen3-coder:free"));
        assert!(result.answers["d/x"].noul > 0.9);
    }

    #[test]
    fn the_typesafe_protocol_is_chosen_over_openai_and_posts_system_ones_body() {
        let (url, request) = serve_once(
            "200 OK",
            r#"{"model":"jev-latest","answers":{"d/x":{"type":"noul","noul":0.81}},"usage":{"tokens":3}}"#,
        );
        let result = Plugin::new(Box::new(Http::new()))
            .classify(params(json!(["openai", "typesafe"]), &url))
            .unwrap();
        let request = request.join().unwrap();

        assert!(
            request.starts_with("POST /v1/systemone HTTP/1.1"),
            "{request}"
        );
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["questions"]["d/x"]["type"], "noul");
        assert!(
            body.get("options").is_none(),
            "options travel beside the body"
        );
        assert_eq!(result.answers["d/x"].noul, 0.81);
        assert_eq!(result.model, "jev-latest");
    }

    #[test]
    fn an_error_body_travels_as_the_data_of_the_error() {
        let (url, _) = serve_once(
            "400 Bad Request",
            r#"{"error":{"message":"logprobs unsupported"}}"#,
        );
        let error = Plugin::new(Box::new(Http::new()))
            .classify(params(json!("openai"), &url))
            .unwrap_err();
        assert!(error.message.contains("400"), "{}", error.message);
        assert_eq!(
            error.data.unwrap()["error"]["message"],
            "logprobs unsupported"
        );
    }

    #[test]
    fn a_provider_declaring_no_served_protocol_is_refused() {
        let error = Plugin::new(Box::new(Http::new()))
            .classify(params(json!("anthropic"), "http://x"))
            .unwrap_err();
        assert!(error.message.contains("serves classification"));
    }
}
