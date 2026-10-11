//! The plugin as the protocol sees it: the handshake, and `classify` over a
//! transport that turns a prompt into text.

use lighthouse_protocol::{
    Answer, ClassifyParams, ClassifyResult, Handler, IndexParams, IndexResult, InitializeParams,
    InitializeResult, ModelManifest, ModelRef, ResultKind, VERSION, response_schema, subjects_text,
    system_text,
};
use serde_json::{Map, Value};

/// The plugin's id, and the id of the one model it offers.
pub const ID: &str = "model-http";
/// Changes with the prompt, so that cached answers of another prompt are not
/// used.
const PROMPT_VERSION: &str = "1";

/// How a provider is reached, from the `options` of its declaration.
#[derive(Debug, PartialEq)]
pub struct Endpoint {
    pub spec: String,
    pub base_url: String,
    pub key_env: Option<String>,
    pub model: String,
}

impl Endpoint {
    /// Reads the options of a request; the model is the part of the reference
    /// after the provider.
    pub fn of(options: &Map<String, Value>) -> Result<Self, String> {
        let text = |key: &str| options.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(Self {
            spec: text("spec").ok_or("the provider declares no `spec`")?,
            base_url: text("baseUrl").ok_or("the provider declares no `baseUrl`")?,
            key_env: text("keyEnv"),
            model: text("model").ok_or("the request names no model")?,
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
            models: vec![ModelManifest {
                id: ID.to_owned(),
                tasks: vec!["classification".to_owned()],
                version: PROMPT_VERSION.to_owned(),
                trains: false,
                deterministic: false,
                abstains: true,
                cost: 1.0,
            }],
        })
    }

    fn index(&mut self, _: IndexParams) -> Result<IndexResult, String> {
        Err("model-http indexes no language".to_owned())
    }

    fn classify(&mut self, params: ClassifyParams) -> Result<ClassifyResult, String> {
        let endpoint = Endpoint::of(&params.options)?;
        let prompt = Prompt {
            system: system_text(&params.decision, &params.decision.examples),
            user: subjects_text(&params.subjects),
            schema: response_schema(),
        };
        let text = self.transport.complete(&endpoint, &prompt)?;
        Ok(ClassifyResult {
            results: answers(&text, &params),
            model: ModelRef {
                id: ID.to_owned(),
                version: format!("{}@{PROMPT_VERSION}", endpoint.model),
            },
        })
    }
}

/// One answer per subject: what the model said where it fits the shape, a
/// `review` where it did not say or said nonsense.
fn answers(text: &str, params: &ClassifyParams) -> Vec<Answer> {
    let given: Vec<Answer> = json_in(text)
        .map(|value| match value {
            Value::Object(mut map) => map.remove("results").unwrap_or(Value::Null),
            other => other,
        })
        .and_then(|list| serde_json::from_value::<Vec<Value>>(list).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| serde_json::from_value::<Answer>(entry).ok())
        .collect();
    params
        .subjects
        .iter()
        .map(|subject| {
            given
                .iter()
                .find(|a| a.key == subject.key)
                .map(|a| Answer {
                    confidence: a.confidence.clamp(0.0, 1.0),
                    ..a.clone()
                })
                .unwrap_or_else(|| Answer {
                    key: subject.key.clone(),
                    kind: ResultKind::Review,
                    confidence: 0.0,
                    reason: "the model gave no usable answer".to_owned(),
                    span: None,
                })
        })
        .collect()
}

/// The JSON in a model's text: the text itself, or what a fenced block holds.
pub fn json_in(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map_or(trimmed, |rest| {
            rest.trim_end().trim_end_matches("```").trim()
        });
    serde_json::from_str(unfenced).ok()
}

#[cfg(test)]
mod tests {
    use lighthouse_protocol::{DecisionRef, Subject, SubjectContext};
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

    fn subject(key: &str) -> Subject {
        Subject {
            key: key.to_owned(),
            kind: "function".to_owned(),
            id: key.to_owned(),
            language: "go".to_owned(),
            file: "a.go".to_owned(),
            text: "func f() {}".to_owned(),
            context: SubjectContext::default(),
        }
    }

    fn params() -> ClassifyParams {
        let options = json!({
            "spec": "openai", "baseUrl": "https://openrouter.ai/api/v1",
            "keyEnv": "OPENROUTER_API_KEY", "model": "qwen/qwen3-coder:free"
        });
        ClassifyParams {
            decision: DecisionRef {
                id: "d/x".to_owned(),
                requirement: "r".to_owned(),
                prompt: "r".to_owned(),
                examples: Vec::new(),
            },
            subjects: vec![subject("a"), subject("b")],
            options: options.as_object().unwrap().clone(),
        }
    }

    fn classify(text: &'static str) -> Vec<Answer> {
        Plugin::new(Box::new(Fixed(text)))
            .classify(params())
            .unwrap()
            .results
    }

    #[test]
    fn a_fenced_answer_is_read_and_a_missing_one_is_left_to_review() {
        let fenced = "```json\n{\"results\":[{\"key\":\"a\",\"kind\":\"fail\",\"confidence\":1.7,\"reason\":\"r\"}]}\n```";
        let results = classify(fenced);
        assert_eq!(results[0].kind, ResultKind::Fail);
        assert_eq!(results[0].confidence, 1.0);
        assert_eq!(results[1].key, "b");
        assert_eq!(results[1].kind, ResultKind::Review);
    }

    #[test]
    fn text_that_is_not_json_leaves_every_subject_to_review() {
        let results = classify("I think a is bad");
        assert!(results.iter().all(|a| a.kind == ResultKind::Review));
    }

    #[test]
    fn the_endpoint_needs_a_spec_a_base_url_and_a_model() {
        assert!(Endpoint::of(&Map::new()).is_err());
        let endpoint = Endpoint::of(&params().options).unwrap();
        assert_eq!(endpoint.key_env.as_deref(), Some("OPENROUTER_API_KEY"));
    }
}
