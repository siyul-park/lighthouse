//! A training file as the learners need it: the text of each labelled
//! subject with its label, weight and group, and the examples the decision's
//! question shows.

use std::collections::BTreeMap;

use lighthouse_protocol::{Question, State, TrainingLine};
use serde_json::Value;

/// One subject: the labelled line of a training file, then embedded for the
/// learners, or the subject a model is asked about.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Line {
    pub text: String,
    /// What the embedder gave for the text; empty until embedded, and for the
    /// rate, which reads nothing.
    pub vector: Vec<f64>,
    /// Maximum and mean cosine to the invalid examples, then to the valid
    /// ones; 0 when a side has none.
    pub sims: [f64; 4],
    /// 1 for a fail, 0 for a pass.
    pub label: f64,
    pub weight: f64,
    pub group: String,
    /// Who gave the label: `person`, `agent` or `model`.
    pub source: String,
}

/// What a training file holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Dataset {
    pub decision: String,
    pub lines: Vec<Line>,
    /// The code of the examples of what fails, and of what passes.
    pub invalid: Vec<String>,
    pub valid: Vec<String>,
}

/// Reads a JSON Lines training file.
pub fn parse(text: &str) -> Result<Dataset, String> {
    let mut lines = Vec::new();
    let mut question: Option<Question> = None;
    let mut decision = String::new();
    for (at, line) in text.lines().enumerate() {
        let read = |e: serde_json::Error| format!("line {}: {e}", at + 1);
        let parsed: TrainingLine = serde_json::from_str(line).map_err(read)?;
        let [asked, state, answer] = &parsed.messages[..] else {
            return Err(format!("line {}: expected three messages", at + 1));
        };
        let state: State = serde_json::from_str(&state.content).map_err(read)?;
        let answer: Value = serde_json::from_str(&answer.content).map_err(read)?;
        if question.is_none() {
            question = Some(serde_json::from_str(&asked.content).map_err(read)?);
            decision.clone_from(&parsed.decision);
        }
        lines.push(Line {
            text: state.text,
            label: f64::from(u8::from(answer["noul"].as_f64().unwrap_or(0.0) >= 0.5)),
            weight: parsed.weight,
            group: parsed.group,
            source: source_of(&parsed.attribution).to_owned(),
            ..Line::default()
        });
    }
    let question = question.ok_or("the training file has no line")?;
    let code = |examples: &[lighthouse_protocol::Example]| {
        examples.iter().map(|e| e.code.clone()).collect()
    };
    Ok(Dataset {
        decision,
        lines,
        invalid: code(&question.criteria.fail.examples),
        valid: code(&question.criteria.pass.examples),
    })
}

/// How many lines each source gave.
pub fn sources(lines: &[Line]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for line in lines {
        *counts.entry(line.source.clone()).or_insert(0) += 1;
    }
    counts
}

/// The kind of source a PROV `wasAttributedTo` object stands for.
fn source_of(attribution: &Value) -> &'static str {
    match (
        attribution["type"].as_str(),
        attribution["hadRole"].as_str(),
    ) {
        (Some("SoftwareAgent"), Some("model")) => "model",
        (Some("SoftwareAgent"), _) => "agent",
        _ => "person",
    }
}

#[cfg(test)]
mod tests;
