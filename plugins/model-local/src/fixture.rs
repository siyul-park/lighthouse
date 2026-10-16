//! A training file for tests, as the host writes it.

use lighthouse_protocol::{DecisionRef, Example, LineKeys, Question, State, TrainingLine};
use serde_json::Value;

/// One labelled subject: its text, whether it fails, its group and PROV
/// attribution (JSON).
pub type Labelled<'a> = (&'a str, bool, &'a str, &'a str);

/// The attribution of a person's label.
pub const PERSON: &str = r#"{"type":"Person"}"#;

fn example(code: &str) -> Example {
    Example {
        language: "go".to_owned(),
        code: code.to_owned(),
        note: String::new(),
    }
}

/// A JSON Lines file of decision `d/x`, whose question shows the examples
/// `bad` and `worse` of a fail and `good` of a pass.
pub fn file(rows: &[Labelled]) -> String {
    let decision = DecisionRef {
        id: "d/x".to_owned(),
        requirement: "r".to_owned(),
        prompt: "r".to_owned(),
        valid: vec![example("good")],
        invalid: vec![example("bad"), example("worse")],
    };
    let question = Question::new(&decision);
    rows.iter()
        .map(|(text, fail, group, attribution)| {
            let state = State {
                language: "go".to_owned(),
                file: "a.go".to_owned(),
                text: (*text).to_owned(),
                facts: Value::Null,
                neighbors: Vec::new(),
            };
            let line = TrainingLine::new(
                &question,
                &state,
                *fail,
                LineKeys {
                    weight: 0.7,
                    group,
                    decision: "d/x",
                    key: "k",
                    attribution: serde_json::from_str(attribution).unwrap(),
                },
            );
            format!("{}\n", serde_json::to_string(&line).unwrap())
        })
        .collect()
}
