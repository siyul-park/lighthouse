//! The text a language model is given to classify subjects, and the shape it
//! must answer in. Shared by the host (for a `command` model, which reads a
//! prompt) and by the plugins that talk to a language model. The shots are a
//! parameter: the decision's own examples, or the nearest judged subjects.

use serde_json::{Value, json};

use crate::{ClassifyParams, DecisionRef, Shot, Subject, Verdict};

/// What the model is told about its job, before the decision.
const INSTRUCTIONS: &str = "You judge one design decision about source code. \
For each subject decide whether it violates the requirement (fail), complies with it (pass), \
or whether the code shown is not enough to tell (abstain). \
Answer only from the code shown. Never take a name alone as evidence. \
Give a confidence from 0 to 1 that your verdict is right, and one sentence of reason. \
Reply with JSON only, no other text: \
{\"results\":[{\"key\":\"<the subject's key>\",\"verdict\":\"fail|pass|abstain\",\"confidence\":0.0,\"reason\":\"...\"}]} \
with exactly one result for each subject.";

/// The system part: the instructions, the requirement and the shots.
pub fn system(decision: &DecisionRef, shots: &[Shot]) -> String {
    let mut out = format!(
        "{INSTRUCTIONS}\n\nDecision: {}\nRequirement: {}\n",
        decision.id, decision.prompt
    );
    for shot in shots {
        let verdict = if shot.verdict == Verdict::Fail {
            "fail"
        } else {
            "pass"
        };
        out.push_str(&format!(
            "\nExample ({verdict}{}):\n```{}\n{}\n```\n",
            if shot.note.is_empty() {
                String::new()
            } else {
                format!(", {}", shot.note)
            },
            shot.language,
            shot.code
        ));
    }
    out
}

/// The user part: the subjects, each with its key, place, facts and source.
pub fn subjects(subjects: &[Subject]) -> String {
    let mut out = String::new();
    for subject in subjects {
        out.push_str(&format!(
            "\nSubject key={} kind={} id={} file={}\n",
            subject.key, subject.kind, subject.id, subject.file
        ));
        if !subject.context.facts.is_null() {
            out.push_str(&format!("facts: {}\n", subject.context.facts));
        }
        for near in &subject.context.neighbors {
            out.push_str(&format!(
                "{} {} {}\n{}\n",
                near.relation, near.kind, near.id, near.text
            ));
        }
        out.push_str(&format!("```{}\n{}\n```\n", subject.language, subject.text));
    }
    out
}

/// One text with everything, for a program that reads a prompt on stdin.
pub fn prompt(params: &ClassifyParams) -> String {
    let mut out = system(&params.decision, &params.decision.examples);
    out.push_str(&subjects(&params.subjects));
    out
}

/// The JSON Schema of the answer: what a structured-output API is asked for.
pub fn response_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "results": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "key": {"type": "string"},
                        "verdict": {"type": "string", "enum": ["fail", "pass", "abstain"]},
                        "confidence": {"type": "number"},
                        "reason": {"type": "string"}
                    },
                    "required": ["key", "verdict", "confidence", "reason"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["results"],
        "additionalProperties": false
    })
}
