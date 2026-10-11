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
/// it, else `openai`. Only `openai` is served so far.
fn spec_for_classification(declared: Option<&Value>) -> Result<String, String> {
    let names: Vec<&str> = match declared {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
        _ => return Err("the provider declares no `spec`".to_owned()),
    };
    if names.contains(&"openai") {
        Ok("openai".to_owned())
    } else {
        Err(format!("no served spec among {names:?} (openai is)"))
    }
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
}
