use std::collections::BTreeMap;

use lighthouse_protocol::{
    Capabilities, DecisionRef, Example, ListModelResponse, Model, Question, ResultKind, State,
    Subject, SubjectContext, Supports, SystemOneRequest, json_in, noul_of, prompt, response_schema,
    subject_text, system_label_text, system_text,
};
use serde_json::json;

fn decision() -> DecisionRef {
    DecisionRef {
        id: "design/x".to_owned(),
        requirement: "A function SHOULD be small.".to_owned(),
        prompt: "Is it small?".to_owned(),
        invalid: vec![Example {
            language: "go".to_owned(),
            code: "func big() {}".to_owned(),
            note: "big".to_owned(),
        }],
        valid: vec![Example {
            language: "go".to_owned(),
            code: "func ok() {}".to_owned(),
            note: String::new(),
        }],
    }
}

fn state() -> State {
    State {
        language: "go".to_owned(),
        file: "a.go".to_owned(),
        text: "func f() {}".to_owned(),
        facts: serde_json::Value::Null,
        neighbors: Vec::new(),
    }
}

#[test]
fn a_question_puts_invalid_examples_under_true_and_valid_ones_under_false() {
    let question = Question::of(&decision());
    let wire = serde_json::to_value(&question).unwrap();

    assert_eq!(wire["type"], "noul");
    assert_eq!(
        wire["criteria"]["true"]["examples"][0]["code"],
        "func big() {}"
    );
    assert_eq!(
        wire["criteria"]["false"]["examples"][0]["code"],
        "func ok() {}"
    );
    assert_eq!(wire["instructions"]["prompt"], "Is it small?");
}

#[test]
fn the_system_text_carries_the_instructions_the_decision_and_its_examples() {
    let text = system_text("design/x", &Question::of(&decision()));
    assert!(text.contains("Is it small?") && text.contains("design/x"));
    assert!(text.contains("Criterion true") && text.contains("func big() {}"));
    assert!(text.contains("noul"));
}

#[test]
fn the_label_system_text_asks_for_a_word_instead_of_a_probability() {
    let question = Question::of(&decision());
    let text = system_label_text("design/x", &question);
    assert!(text.contains("Answer with one word") && text.contains("func big() {}"));
    assert!(!text.contains("noul") && !text.contains("JSON"));
    // Only the reply instruction differs.
    let rest = |text: &str| {
        text.split_once("\n\nDecision")
            .map(|(_, rest)| rest.to_owned())
    };
    assert_eq!(rest(&text), rest(&system_text("design/x", &question)));
}

#[test]
fn the_subject_text_shows_the_file_and_the_source() {
    let text = subject_text(&state());
    assert!(text.contains("a.go") && text.contains("func f() {}"));
}

#[test]
fn a_prompt_holds_both_and_the_schema_asks_for_a_probability() {
    let request = SystemOneRequest {
        state: state(),
        model: "m".to_owned(),
        questions: BTreeMap::from([("design/x".to_owned(), Question::of(&decision()))]),
        options: Default::default(),
    };
    let text = prompt(&request);
    assert!(text.contains("Is it small?") && text.contains("func f() {}"));
    assert_eq!(response_schema()["properties"]["noul"]["type"], "number");
}

#[test]
fn system_one_options_are_the_only_optional_top_level_request_key() {
    let mut request = SystemOneRequest {
        state: state(),
        model: "m".to_owned(),
        questions: BTreeMap::from([("design/x".to_owned(), Question::of(&decision()))]),
        options: Default::default(),
    };
    let keys = |value: &serde_json::Value| {
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort_unstable();
        keys
    };
    assert_eq!(
        keys(&serde_json::to_value(&request).unwrap()),
        ["model", "questions", "state"]
    );

    request
        .options
        .insert("baseUrl".to_owned(), json!("https://example"));
    assert_eq!(
        keys(&serde_json::to_value(&request).unwrap()),
        ["model", "options", "questions", "state"]
    );
}

#[test]
fn a_probability_is_read_from_a_system_one_body_or_a_bare_answer() {
    let body = json!({"model": "m", "answers": {"d": {"type": "noul", "noul": 0.25}}});
    assert_eq!(noul_of(&body, "d"), Some(0.25));
    assert_eq!(noul_of(&json!({"noul": 0.5}), "d"), Some(0.5));
    assert_eq!(noul_of(&json!({"noul": 1.5}), "d"), None);
    assert_eq!(noul_of(&body, "other"), None);
}

#[test]
fn a_model_list_is_an_openai_list_object_with_lighthouse_capabilities() {
    let model = Model {
        id: "m".to_owned(),
        object: "model".to_owned(),
        created: 0,
        owned_by: "m".to_owned(),
        capabilities: Capabilities {
            classification: Some(Supports { supported: true }),
            embedding: None,
            fine_tuning: Some(Supports { supported: true }),
        },
        version: "1".to_owned(),
        deterministic: false,
        abstains: true,
        cost: 2.0,
    };
    let list = ListModelResponse {
        object: "list".to_owned(),
        data: vec![model],
    };
    let wire = serde_json::to_value(&list).unwrap();

    assert_eq!(wire["object"], "list");
    assert_eq!(wire["data"][0]["object"], "model");
    assert_eq!(
        wire["data"][0]["capabilities"]["fine_tuning"]["supported"],
        true
    );
    assert!(wire["data"][0]["capabilities"].get("embedding").is_none());
    assert_eq!(list.data[0].id, "m");
    assert!(ListModelResponse::default().is_empty());
}

#[test]
fn a_subject_shows_a_model_its_state_and_json_may_come_fenced() {
    let subject = Subject {
        key: "k".to_owned(),
        kind: "function".to_owned(),
        id: "m::f".to_owned(),
        language: "go".to_owned(),
        file: "a.go".to_owned(),
        text: "func f() {}".to_owned(),
        context: SubjectContext::default(),
    };
    assert_eq!(subject.state().text, "func f() {}");
    assert_eq!(
        json_in("```json\n{\"noul\": 0.5}\n```"),
        Some(json!({"noul": 0.5}))
    );
    assert_eq!(json_in("{\"noul\": 0.5}"), Some(json!({"noul": 0.5})));
    assert_eq!(json_in("no"), None);
}

#[test]
fn a_training_line_is_openais_supervised_format_with_lighthouses_keys() {
    use lighthouse_protocol::{LineKeys, TrainingLine};

    let state = State {
        language: "go".to_owned(),
        file: "a.go".to_owned(),
        text: "func f() {}".to_owned(),
        facts: json!({}),
        neighbors: Vec::new(),
    };
    let question = Question::of(&decision());
    let line = |label| {
        let keys = LineKeys {
            weight: 0.5,
            group: "a.go",
            decision: "design/x",
            key: "k",
            attribution: json!({"type": "Person"}),
        };
        let line = TrainingLine::new(&question, &state, label, keys);
        serde_json::to_value(line).unwrap()
    };
    let fail = line(ResultKind::Fail);
    let roles: Vec<&str> = fail["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();

    assert_eq!(roles, ["system", "user", "assistant"]);
    let content = |line: &serde_json::Value, at: usize| -> serde_json::Value {
        serde_json::from_str(line["messages"][at]["content"].as_str().unwrap()).unwrap()
    };
    assert_eq!(content(&fail, 0)["type"], "noul");
    assert_eq!(content(&fail, 1)["text"], "func f() {}");
    assert_eq!(content(&fail, 2), json!({"type": "noul", "noul": 1}));
    assert_eq!(
        content(&line(ResultKind::Pass), 2),
        json!({"type": "noul", "noul": 0})
    );
    assert_eq!(fail["weight"], 0.5);
    assert_eq!(fail["group"], "a.go");
    assert_eq!(fail["decision"], "design/x");
    assert_eq!(fail["key"], "k");
    assert_eq!(fail["attribution"]["type"], "Person");
}
