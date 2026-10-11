use lighthouse_protocol::{
    ClassifyParams, DecisionRef, Message, ResultKind, Shot, Subject, SubjectContext, prompt,
    response_schema, subjects_text, system_text,
};

fn decision() -> DecisionRef {
    DecisionRef {
        id: "design/x".to_owned(),
        requirement: "A function SHOULD be small.".to_owned(),
        prompt: "Is it small?".to_owned(),
        examples: vec![Shot {
            kind: ResultKind::Fail,
            language: "go".to_owned(),
            code: "func big() {}".to_owned(),
            note: "big".to_owned(),
        }],
    }
}

fn subject() -> Subject {
    Subject {
        key: "k1".to_owned(),
        kind: "function".to_owned(),
        id: "m::f#function".to_owned(),
        language: "go".to_owned(),
        file: "a.go".to_owned(),
        text: "func f() {}".to_owned(),
        context: SubjectContext::default(),
    }
}

#[test]
fn the_system_text_carries_the_instructions_the_decision_and_its_shots() {
    let decision = decision();
    let text = system_text(&decision, &decision.examples);
    assert!(text.contains("Is it small?") && text.contains("design/x"));
    assert!(text.contains("Example (fail, big)") && text.contains("func big() {}"));
    assert!(text.contains("review"));
    assert!(!system_text(&decision, &[]).contains("Example"));
}

#[test]
fn the_subjects_text_names_each_key_and_shows_its_source() {
    let text = subjects_text(&[subject()]);
    assert!(text.contains("key=k1") && text.contains("func f() {}"));
}

#[test]
fn a_prompt_holds_both_and_the_schema_asks_for_kinds_and_confidence() {
    let params = ClassifyParams {
        decision: decision(),
        subjects: vec![subject()],
        options: serde_json::Map::new(),
    };
    let text = prompt(&params);
    assert!(text.contains("Is it small?") && text.contains("key=k1"));
    let schema = response_schema();
    let kinds = &schema["properties"]["results"]["items"]["properties"]["kind"]["enum"];
    assert_eq!(kinds, &serde_json::json!(["fail", "pass", "review"]));
}

#[test]
fn an_error_carries_its_canonical_status() {
    let failure = Message::failure(None, -32602, "bad").with_status("INVALID_ARGUMENT");
    assert_eq!(
        failure.error.unwrap().data.unwrap()["status"],
        "INVALID_ARGUMENT"
    );
}
