use std::collections::BTreeMap;

use lighthouse_protocol::{
    Answer, Capabilities, DecisionRef, Example, ListModelResponse, Model, NOUL, Question, State,
    Subject, SystemOneRequest, artifact_path, content_dir, dataset_path,
    prompt::{self, json_in, noul_of},
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
    let question = Question::new(&decision());
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
fn the_system_text_carries_the_instructions_the_decision_and_its_examples_and_asks_for_a_word() {
    let text = prompt::system("design/x", &Question::new(&decision()));
    assert!(text.contains("Is it small?") && text.contains("design/x"));
    assert!(text.contains("Criterion true") && text.contains("func big() {}"));
    assert!(text.contains("Answer with one word"));
    assert!(!text.contains("noul") && !text.contains("JSON"));
}

#[test]
fn the_subject_text_shows_the_file_and_the_source() {
    let text = prompt::subject(&state());
    assert!(text.contains("a.go") && text.contains("func f() {}"));
}

#[test]
fn system_one_options_are_the_only_optional_top_level_request_key() {
    let mut request = SystemOneRequest {
        state: state(),
        model: "m".to_owned(),
        questions: BTreeMap::from([("design/x".to_owned(), Question::new(&decision()))]),
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
        abstains: true,
        cost: 2.0,
        ..Model::new("m", "1", Capabilities::classifier(true))
    };
    let list = ListModelResponse::new(vec![model]);
    let wire = serde_json::to_value(&list).unwrap();

    assert_eq!(wire["object"], "list");
    assert_eq!(wire["data"][0]["object"], "model");
    assert_eq!(
        wire["data"][0]["capabilities"]["training"]["supported"],
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
        state: state(),
    };
    assert_eq!(subject.state.text, "func f() {}");
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
    let question = Question::new(&decision());
    let line = |fail| {
        let keys = LineKeys {
            weight: 0.5,
            group: "a.go",
            decision: "design/x",
            key: "k",
            attribution: json!({"type": "Person"}),
        };
        let line = TrainingLine::new(&question, &state, fail, keys);
        serde_json::to_value(line).unwrap()
    };
    let fail = line(true);
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
    assert_eq!(content(&line(false), 2), json!({"type": "noul", "noul": 0}));
    assert_eq!(fail["weight"], 0.5);
    assert_eq!(fail["group"], "a.go");
    assert_eq!(fail["decision"], "design/x");
    assert_eq!(fail["key"], "k");
    assert_eq!(fail["attribution"]["type"], "Person");
}

#[test]
fn an_answer_is_a_noul_and_the_store_names_files_by_digest() {
    let answer = serde_json::to_value(Answer::new(0.25)).unwrap();
    assert_eq!(answer, json!({"type": NOUL, "noul": 0.25}));

    let dir = std::path::Path::new("/p/.lighthouse/models");
    assert_eq!(
        artifact_path(dir, "ab"),
        std::path::Path::new("/p/.lighthouse/models/sha256/ab.json")
    );
    assert_eq!(
        dataset_path(dir, "ab"),
        std::path::Path::new("/p/.lighthouse/models/sha256/ab.jsonl")
    );
}

#[test]
fn content_dir_is_the_sha256_directory_the_store_files_live_in() {
    let dir = std::path::Path::new("/p/.lighthouse/models");
    assert_eq!(
        content_dir(dir),
        std::path::Path::new("/p/.lighthouse/models/sha256")
    );
}

#[test]
fn capabilities_learns_only_when_fine_tuning_is_supported() {
    assert!(Capabilities::classifier(true).learns());
    assert!(!Capabilities::classifier(false).learns());
    assert!(!Capabilities::default().learns());
}
