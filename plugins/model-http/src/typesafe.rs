//! TypeSafe's API (https://docs.typesafe.ai/api). One function per operation
//! used. The docs title the endpoint "evaluation" and give no operationId, so
//! the path names it: `systemone`. The bodies are the protocol's
//! [`SystemOneRequest`] and [`SystemOneResponse`], System One's own.

use lighthouse_protocol::{ErrorObject, SYSTEM_ONE, SystemOneRequest, SystemOneResponse};

use crate::{endpoint::Endpoint, http::Client};

/// [`SYSTEM_ONE`]: the questions put to the model, answered as probabilities.
pub fn systemone(
    client: &dyn Client,
    endpoint: &Endpoint,
    request: &SystemOneRequest,
) -> Result<SystemOneResponse, ErrorObject> {
    let mut api_request = request.clone();
    api_request.options.clear();
    let body = serde_json::to_value(api_request).map_err(|e| e.to_string())?;
    let response = client.post(
        &endpoint.url(SYSTEM_ONE.path),
        endpoint.key()?.as_deref(),
        &body,
        endpoint.timeout,
    )?;
    serde_json::from_value(response)
        .map_err(|e| format!("the response does not fit System One's: {e}").into())
}

#[cfg(test)]
mod tests {
    use lighthouse_protocol::{Criteria, Criterion, Instructions, Question, State};
    use serde_json::{Value, json};

    use super::*;
    use crate::recording::Recording;

    /// The request of the System One reference.
    const REQUEST: &str = r#"{"model":"jev-latest","state":"Customer: I want to speak to a person.","questions":{"is_human_escalation":{"type":"noul","instructions":"Is the customer asking for a human?","criteria":{"true":"They ask for a person.","false":"They do not."}}}}"#;
    /// The response of the System One reference.
    const WITH_MODEL: &str = r#"{"model":"jev-1.13.0","answers":{"is_human_escalation":{"type":"noul","noul":0.99},"is_repeat_contact":{"type":"noul","noul":0.93}},"usage":{"input_tokens":360,"output_tokens":39}}"#;
    /// The shortest answer the reference shows: no model, no type.
    const BARE: &str =
        r#"{"answers":{"is_human_escalation":{"noul":0.98},"is_repeat_contact":{"noul":0.95}}}"#;

    fn request() -> SystemOneRequest {
        let side = |meaning: &str| Criterion {
            meaning: meaning.to_owned(),
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
            model: "jev-latest".to_owned(),
            questions: [(
                "d/x".to_owned(),
                Question {
                    kind: "noul".to_owned(),
                    instructions: Instructions {
                        requirement: "r".to_owned(),
                        prompt: "r".to_owned(),
                    },
                    criteria: Criteria {
                        fail: side("bad"),
                        pass: side("good"),
                    },
                },
            )]
            .into(),
            options: Default::default(),
        }
    }

    #[test]
    fn it_posts_the_body_to_the_systemone_path() {
        let client = Recording::replying(vec![serde_json::from_str(WITH_MODEL).unwrap()]);
        let endpoint = Endpoint::at("typesafe", "https://api.typesafe.ai");
        let mut request = request();
        request
            .options
            .insert("token".to_owned(), Value::String("secret".to_owned()));
        systemone(&client, &endpoint, &request).unwrap();
        let asked = client.asked();
        assert_eq!(asked[0].0, "https://api.typesafe.ai/v1/systemone");
        assert_eq!(asked[0].1["model"], "jev-latest");
        assert!(asked[0].1.get("options").is_none());
        assert_eq!(asked[0].1["questions"]["d/x"]["type"], "noul");
        assert_eq!(
            asked[0].1["questions"]["d/x"]["criteria"]["true"]["meaning"],
            "bad"
        );
    }

    #[test]
    fn the_documented_answers_are_read_with_or_without_model_and_type() {
        let endpoint = Endpoint::at("typesafe", "https://x");
        let read = |text: &str| {
            let client = Recording::replying(vec![serde_json::from_str(text).unwrap()]);
            systemone(&client, &endpoint, &request()).unwrap()
        };
        let full = read(WITH_MODEL);
        assert_eq!(full.model, "jev-1.13.0");
        assert_eq!(full.answers["is_human_escalation"].noul, 0.99);
        assert_eq!(full.answers["is_human_escalation"].kind, "noul");
        let bare = read(BARE);
        assert_eq!(bare.answers["is_repeat_contact"].noul, 0.95);
        assert_eq!(bare.answers["is_repeat_contact"].kind, "noul");
    }

    #[test]
    fn the_documented_request_has_the_keys_ours_has() {
        let documented: Value = serde_json::from_str(REQUEST).unwrap();
        let keys = |v: &Value| {
            let mut keys: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        let ours = serde_json::to_value(request()).unwrap();
        assert_eq!(keys(&documented), keys(&ours));
        assert_eq!(
            documented["questions"]["is_human_escalation"]["type"],
            "noul"
        );
    }

    #[test]
    fn an_answer_of_another_shape_fails() {
        let client = Recording::replying(vec![json!({"answers": 3})]);
        let endpoint = Endpoint::at("typesafe", "https://x");
        assert!(systemone(&client, &endpoint, &request()).is_err());
    }
}
