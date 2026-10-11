//! The plugin as the protocol sees it: the handshake, and `classify` over a
//! transport that turns a prompt into text. The request is TypeSafe System
//! One's body; for a chat API each question is rendered as a prompt that asks
//! for the probability of yes, and the answers come back in System One's
//! shape.

use std::collections::BTreeMap;

use lighthouse_protocol::{
    ClassifyParams, ClassifyResult, Handler, IndexParams, IndexResult, InitializeParams,
    InitializeResult, ModelList, ModelManifest, QuestionAnswer, VERSION, json_in, noul_of,
    response_schema, subject_text, system_text,
};
use serde_json::{Map, Value};

/// The plugin's id, and the id of the one model it offers.
pub const ID: &str = "model-http";
/// Changes with the prompt, so that cached answers of another prompt are not
/// used.
const PROMPT_VERSION: &str = "2";

/// How a provider is reached, from the `options` of its declaration and the
/// model of the request.
#[derive(Debug, PartialEq)]
pub struct Endpoint {
    pub spec: String,
    pub base_url: String,
    pub key_env: Option<String>,
    pub model: String,
}

impl Endpoint {
    /// Reads the options of a provider and the model of a request.
    pub fn of(options: &Map<String, Value>, model: &str) -> Result<Self, String> {
        let text = |key: &str| options.get(key).and_then(Value::as_str).map(str::to_owned);
        if model.is_empty() {
            return Err("the request names no model".to_owned());
        }
        Ok(Self {
            spec: spec_for_classification(options.get("spec"))?,
            base_url: text("baseUrl").ok_or("the provider declares no `baseUrl`")?,
            key_env: text("keyEnv"),
            model: model.to_owned(),
        })
    }
}

/// One request for text: the system and user parts, and the shape the answer
/// must have.
pub struct Prompt {
    pub system: String,
    pub user: String,
    pub schema: Value,
}

/// Something that turns a prompt into the model's text.
pub trait Transport {
    fn complete(&self, endpoint: &Endpoint, prompt: &Prompt) -> Result<String, String>;

    /// Forwards the System One body as it is.
    fn systemone(
        &self,
        endpoint: &Endpoint,
        params: &ClassifyParams,
    ) -> Result<ClassifyResult, String>;
}

/// The transports of the specs served: each operation goes to the spec that
/// covers it.
pub struct Specs;

impl Transport for Specs {
    fn complete(&self, endpoint: &Endpoint, prompt: &Prompt) -> Result<String, String> {
        crate::openai::Http.complete(endpoint, prompt)
    }

    fn systemone(
        &self,
        endpoint: &Endpoint,
        params: &ClassifyParams,
    ) -> Result<ClassifyResult, String> {
        crate::typesafe::classify(endpoint, params)
    }
}

pub struct Plugin {
    transport: Box<dyn Transport>,
}

impl Plugin {
    pub fn new(transport: Box<dyn Transport>) -> Self {
        Self { transport }
    }
}

impl Handler for Plugin {
    fn initialize(&mut self, params: InitializeParams) -> Result<InitializeResult, String> {
        if params.protocol_version != VERSION {
            return Err(format!("speaks protocol {VERSION}"));
        }
        Ok(InitializeResult {
            id: ID.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: VERSION.to_owned(),
            languages: Vec::new(),
            models: ModelList::of(&[ModelManifest {
                id: ID.to_owned(),
                tasks: vec!["classification".to_owned()],
                version: PROMPT_VERSION.to_owned(),
                trains: false,
                deterministic: false,
                abstains: true,
                cost: 1.0,
            }]),
        })
    }

    fn index(&mut self, _: IndexParams) -> Result<IndexResult, String> {
        Err("model-http indexes no language".to_owned())
    }

    fn classify(&mut self, params: ClassifyParams) -> Result<ClassifyResult, String> {
        let endpoint = Endpoint::of(&params.options, &params.model)?;
        if endpoint.spec == "typesafe" {
            return self.transport.systemone(&endpoint, &params);
        }
        let mut answers = BTreeMap::new();
        for (id, question) in &params.questions {
            let prompt = Prompt {
                system: system_text(id, question),
                user: subject_text(&params.state),
                schema: response_schema(),
            };
            let text = self.transport.complete(&endpoint, &prompt)?;
            if let Some(noul) = json_in(&text).and_then(|value| noul_of(&value, id)) {
                answers.insert(
                    id.clone(),
                    QuestionAnswer {
                        kind: "noul".to_owned(),
                        noul,
                    },
                );
            }
        }
        Ok(ClassifyResult {
            model: format!("{}@{PROMPT_VERSION}", endpoint.model),
            answers,
            usage: Value::Null,
        })
    }
}

/// The spec that covers classification: `typesafe` when the provider speaks
/// it, else `openai`.
fn spec_for_classification(declared: Option<&Value>) -> Result<String, String> {
    let names: Vec<&str> = match declared {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
        _ => return Err("the provider declares no `spec`".to_owned()),
    };
    ["typesafe", "openai"]
        .into_iter()
        .find(|spec| names.contains(spec))
        .map(str::to_owned)
        .ok_or_else(|| format!("no served spec among {names:?} (openai and typesafe are)"))
}

#[cfg(test)]
mod tests {
    use lighthouse_protocol::{Criteria, Criterion, Instructions, Question, State};
    use serde_json::json;

    use super::*;

    /// Answers with a fixed text, whatever it is asked.
    struct Fixed(&'static str);

    impl Transport for Fixed {
        fn complete(&self, endpoint: &Endpoint, _: &Prompt) -> Result<String, String> {
            assert_eq!(endpoint.model, "qwen/qwen3-coder:free");
            Ok(self.0.to_owned())
        }

        fn systemone(&self, _: &Endpoint, _: &ClassifyParams) -> Result<ClassifyResult, String> {
            Err("not asked".to_owned())
        }
    }

    fn params() -> ClassifyParams {
        let options = json!({
            "spec": "openai", "baseUrl": "https://openrouter.ai/api/v1",
            "keyEnv": "OPENROUTER_API_KEY"
        });
        let side = || Criterion {
            meaning: "m".to_owned(),
            examples: Vec::new(),
        };
        ClassifyParams {
            state: State {
                language: "go".to_owned(),
                file: "a.go".to_owned(),
                text: "func f() {}".to_owned(),
                facts: Value::Null,
                neighbors: Vec::new(),
            },
            model: "qwen/qwen3-coder:free".to_owned(),
            questions: BTreeMap::from([(
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
            )]),
            options: options.as_object().unwrap().clone(),
        }
    }

    fn classify(text: &'static str) -> ClassifyResult {
        Plugin::new(Box::new(Fixed(text)))
            .classify(params())
            .unwrap()
    }

    #[test]
    fn a_fenced_probability_is_the_answer_in_system_ones_shape() {
        let result = classify("```json\n{\"noul\": 0.93}\n```");
        assert_eq!(result.answers["d/x"].noul, 0.93);
        assert_eq!(result.answers["d/x"].kind, "noul");
        assert!(result.model.starts_with("qwen/qwen3-coder:free@"));
    }

    #[test]
    fn text_that_is_not_a_probability_is_no_answer() {
        assert!(classify("I think it is bad").answers.is_empty());
        assert!(classify("{\"noul\": 7}").answers.is_empty());
    }

    #[test]
    fn the_endpoint_needs_a_spec_a_base_url_and_a_model() {
        assert!(Endpoint::of(&Map::new(), "m").is_err());
        assert!(Endpoint::of(&params().options, "").is_err());
        let endpoint = Endpoint::of(&params().options, "m").unwrap();
        assert_eq!(endpoint.key_env.as_deref(), Some("OPENROUTER_API_KEY"));
    }

    /// Serves one canned JSON response on a local port and returns the base
    /// URL and what the request was.
    fn serve_once(body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
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
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (url, handle)
    }

    fn over_http(spec: Value, url: &str) -> ClassifyResult {
        let mut params = params();
        params.options = json!({ "spec": spec, "baseUrl": url })
            .as_object()
            .unwrap()
            .clone();
        Plugin::new(Box::new(Specs)).classify(params).unwrap()
    }

    #[test]
    fn the_openai_spec_posts_chat_completions_with_the_schema_and_reads_the_probability() {
        let (url, request) = serve_once(
            r#"{"choices":[{"message":{"role":"assistant","content":"{\"noul\": 0.97}"}}]}"#,
        );
        let result = over_http(json!("openai"), &url);
        let request = request.join().unwrap();

        assert!(
            request.starts_with("POST /chat/completions HTTP/1.1"),
            "{request}"
        );
        assert!(request.contains("json_schema") && request.contains("noul"));
        assert!(request.contains("qwen/qwen3-coder:free"));
        assert_eq!(result.answers["d/x"].noul, 0.97);
    }

    #[test]
    fn the_typesafe_spec_forwards_system_ones_body_to_its_endpoint() {
        let (url, request) = serve_once(
            r#"{"model":"jev-latest","answers":{"d/x":{"type":"noul","noul":0.81}},"usage":{"tokens":3}}"#,
        );
        let result = over_http(json!(["openai", "typesafe"]), &url);
        let request = request.join().unwrap();

        assert!(
            request.starts_with("POST /v1/systemone HTTP/1.1"),
            "{request}"
        );
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["model"], "qwen/qwen3-coder:free");
        assert_eq!(body["state"]["text"], "func f() {}");
        assert_eq!(body["questions"]["d/x"]["type"], "noul");
        assert_eq!(result.answers["d/x"].noul, 0.81);
        assert_eq!(result.model, "jev-latest");
    }
}
