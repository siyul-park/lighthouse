//! A training file for tests, as the host writes it.

use lighthouse_protocol::{
    DecisionRef, Example, LineKeys, Question, ResultKind, Subject, TrainingLine,
};
use serde_json::json;

/// One labelled subject: its text, label, group and PROV attribution (JSON).
pub type Labelled<'a> = (&'a str, ResultKind, &'a str, &'a str);

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
    let question = Question::of(&decision);
    rows.iter()
        .map(|(text, label, group, attribution)| {
            let subject: Subject = serde_json::from_value(json!({
                "key": "k", "kind": "function", "id": "a", "language": "go",
                "file": "a.go", "text": text
            }))
            .unwrap();
            let line = TrainingLine::new(
                &question,
                &subject.state(),
                *label,
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
