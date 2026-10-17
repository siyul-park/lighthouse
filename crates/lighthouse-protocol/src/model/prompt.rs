//! The text a language model is given to answer a System One question, and
//! how to read an answer that a program printed. The question is answered by
//! the model's own distribution over `true` and `false`, never by a number it
//! writes. Shared by the plugins that talk to a language model and by the host
//! (for a `command` model).

use serde_json::Value;

use crate::{Question, State};

/// What the model is told about its job.
const JOB: &str = "You judge one design decision about source code. \
You are given a subject and a question: does the subject violate the requirement \
(the criterion `true`) or comply with it (the criterion `false`)? \
Answer only from the code shown. Never take a name alone as evidence.";

/// How it is told to reply: its first token is the answer.
const REPLY: &str = "Answer with one word: `true` if the subject violates the requirement \
(the criterion `true`), `false` if it complies.";

/// The system part for one question: the instructions, the requirement, the
/// check's prompt when it differs from the requirement, and the examples of
/// both criteria. The model answers with one word, `true` or `false`.
pub fn system(id: &str, question: &Question) -> String {
    let instructions = &question.instructions;
    let mut out = format!(
        "{JOB} {REPLY}\n\nDecision: {id}\nRequirement: {}\n",
        instructions.requirement
    );
    if instructions.prompt != instructions.requirement {
        out.push_str(&format!("Prompt: {}\n", instructions.prompt));
    }
    for (criterion, side) in [
        ("true", &question.criteria.fail),
        ("false", &question.criteria.pass),
    ] {
        out.push_str(&format!("\nCriterion {criterion}: {}\n", side.meaning));
        for example in &side.examples {
            let note = if example.note.is_empty() {
                String::new()
            } else {
                format!(" ({})", example.note)
            };
            out.push_str(&format!(
                "Example{note}:\n```{}\n{}\n```\n",
                example.language, example.code
            ));
        }
    }
    out
}

/// The user part: the subject's place, facts, neighbors and source.
pub fn subject(state: &State) -> String {
    let mut out = format!("Subject in {}\n", state.file);
    if !state.facts.is_null() {
        out.push_str(&format!("facts: {}\n", state.facts));
    }
    for near in &state.neighbors {
        out.push_str(&format!(
            "{} {} {}\n{}\n",
            near.relation, near.kind, near.id, near.text
        ));
    }
    out.push_str(&format!("```{}\n{}\n```\n", state.language, state.text));
    out
}

/// The probability of yes that a program printed for question `id`: the
/// System One body (`answers.<id>.noul`), or just `{"noul": p}`. Anything
/// else, or a value outside 0 to 1, is no answer.
pub fn noul_of(printed: &Value, id: &str) -> Option<f64> {
    let p = printed
        .get("answers")
        .and_then(|answers| answers.get(id))
        .and_then(|answer| answer.get("noul"))
        .or_else(|| printed.get("noul"))?
        .as_f64()?;
    (0.0..=1.0).contains(&p).then_some(p)
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
