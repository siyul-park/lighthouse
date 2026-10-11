//! Dispatch is data: for each protocol method, the functions that can serve
//! it, the method's own API first. A call is served by the first entry whose
//! spec the provider declares; nothing else compares spec names.

use lighthouse_protocol::{HandlerError, SystemOneRequest, SystemOneResponse};

use crate::{chat, endpoint::Endpoint, http::Client, typesafe};

/// `lighthouse/classify`: System One itself, then Chat Completions by
/// adaptation.
const CLASSIFY: [(&str, Systemone); 2] = [
    ("typesafe", typesafe::systemone),
    ("openai", chat::systemone),
];

/// A function that puts the questions of a System One request to a model.
pub type Systemone =
    fn(&dyn Client, &Endpoint, &SystemOneRequest) -> Result<SystemOneResponse, HandlerError>;

/// The function that serves `lighthouse/classify` for `endpoint`.
pub fn classify(endpoint: &Endpoint) -> Result<Systemone, HandlerError> {
    CLASSIFY
        .iter()
        .find(|(spec, _)| endpoint.speaks(spec))
        .map(|(_, serve)| *serve)
        .ok_or_else(|| {
            let served: Vec<&str> = CLASSIFY.iter().map(|(spec, _)| *spec).collect();
            format!(
                "no spec of {:?} serves classification (served: {served:?})",
                endpoint.specs
            )
            .into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::Recording;

    fn declaring(specs: &[&str]) -> Endpoint {
        Endpoint {
            specs: specs.iter().map(|s| (*s).to_owned()).collect(),
            base_url: "https://x".to_owned(),
            key_env: None,
        }
    }

    /// The path the chosen function posts to, whatever it answers.
    fn path_for(specs: &[&str]) -> String {
        use lighthouse_protocol::{State, SystemOneRequest};

        let endpoint = declaring(specs);
        let client = Recording::replying(Vec::new());
        let request = SystemOneRequest {
            state: State {
                language: "go".to_owned(),
                file: "a.go".to_owned(),
                text: String::new(),
                facts: serde_json::Value::Null,
                neighbors: Vec::new(),
            },
            model: "m".to_owned(),
            questions: [(
                "d/x".to_owned(),
                lighthouse_protocol::Question::of(&lighthouse_protocol::DecisionRef {
                    id: "d/x".to_owned(),
                    requirement: "r".to_owned(),
                    prompt: "r".to_owned(),
                    examples: Vec::new(),
                }),
            )]
            .into(),
        };
        let _ = classify(&endpoint).unwrap()(&client, &endpoint, &request);
        client.asked()[0].0.clone()
    }

    #[test]
    fn a_method_goes_to_the_first_entry_the_provider_declares() {
        assert_eq!(path_for(&["openai", "typesafe"]), "https://x/v1/systemone");
        assert_eq!(path_for(&["openai"]), "https://x/chat/completions");
    }

    #[test]
    fn a_provider_declaring_no_served_spec_is_refused() {
        assert!(classify(&declaring(&["anthropic"])).is_err());
    }
}
