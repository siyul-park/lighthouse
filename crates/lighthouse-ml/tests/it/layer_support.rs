//! Fakes for grading a layer: models, runs and labels.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use lighthouse_engine::Run;
use lighthouse_ml::{
    evaluate::{EvalReport, Found, Overlaid, Overlay, grade},
    labels::{Label, Source},
    metrics::Outcome,
};
use lighthouse_model::{Diagnostic, Fingerprint, Judgment, Position, Result, Severity, Span};
use lighthouse_plugin::{
    Answer, Capabilities, CreateFineTuningJobRequest, DecisionRef, Error, FineTuningJob,
    FineTuningJobStatus, Model, ModelClient, State, Subject, Supports, SystemOneRequest,
    SystemOneResponse,
};
use lighthouse_protocol::hash::sha256;
use lighthouse_spec::Evaluation;
use serde_json::{Value, json};

pub const RULE: &str = "core/file-size";

pub fn decision() -> DecisionRef {
    DecisionRef {
        id: RULE.to_owned(),
        requirement: "r".to_owned(),
        prompt: "r".to_owned(),
        valid: Vec::new(),
        invalid: Vec::new(),
    }
}

pub fn manifest(deterministic: bool) -> Model {
    Model {
        id: "fake".to_owned(),
        object: "model".to_owned(),
        created: 0,
        owned_by: "fake".to_owned(),
        capabilities: Capabilities {
            classification: Some(Supports { supported: true }),
            fine_tuning: Some(Supports { supported: true }),
            ..Capabilities::default()
        },
        version: "1".to_owned(),
        deterministic,
        abstains: false,
        cost: 0.0,
    }
}

pub fn answers(params: &SystemOneRequest, noul: f64) -> SystemOneResponse {
    SystemOneResponse {
        model: String::new(),
        answers: params
            .questions
            .keys()
            .map(|id| {
                let answer = Answer {
                    kind: "noul".to_owned(),
                    noul,
                };
                (id.clone(), answer)
            })
            .collect(),
        usage: Value::Null,
    }
}

/// A model that says what a table of the text of subjects says; a subject
/// that is not in it is a pass.
pub struct Table(pub Model, pub BTreeMap<String, f64>);

impl ModelClient for Table {
    fn manifest(&self) -> &Model {
        &self.0
    }

    fn classify(&self, params: &SystemOneRequest) -> std::result::Result<SystemOneResponse, Error> {
        let noul = self.1.get(&params.state.text).copied().unwrap_or(0.0);
        Ok(answers(params, noul))
    }

    fn train(&self, _: &CreateFineTuningJobRequest) -> std::result::Result<FineTuningJob, Error> {
        Err(Error::Failed("no".to_owned()))
    }
}

pub fn table(rows: &[(&str, f64)]) -> Table {
    Table(
        manifest(true),
        rows.iter().map(|(t, p)| ((*t).to_owned(), *p)).collect(),
    )
}

/// A trainer whose artifact memorises the text of the subjects it was given:
/// it answers 1.0 for one it has seen and 0.0 for any other.
pub struct Memorizer(pub Model);

impl ModelClient for Memorizer {
    fn manifest(&self) -> &Model {
        &self.0
    }

    fn classify(&self, _: &SystemOneRequest) -> std::result::Result<SystemOneResponse, Error> {
        Err(Error::Failed("a trainer does not classify".to_owned()))
    }

    fn train(
        &self,
        request: &CreateFineTuningJobRequest,
    ) -> std::result::Result<FineTuningJob, Error> {
        let dir = |name: &str| PathBuf::from(&request.metadata[name]);
        let hex = request.training_file.strip_prefix("sha256:").unwrap();
        let text = fs::read_to_string(dir("datasets").join("sha256").join(format!("{hex}.jsonl")))
            .map_err(|e| Error::Failed(e.to_string()))?;
        let seen: BTreeSet<String> = text.lines().map(memorised).collect();
        let artifact = json!({
            "seen": seen,
            "card": {"trainingFile": request.training_file},
        })
        .to_string();
        let digest = sha256(&artifact);
        let models = dir("dir").join("sha256");
        fs::create_dir_all(&models).map_err(|e| Error::Failed(e.to_string()))?;
        fs::write(models.join(format!("{digest}.json")), artifact)
            .map_err(|e| Error::Failed(e.to_string()))?;
        Ok(FineTuningJob {
            id: "job".to_owned(),
            created_at: 0,
            finished_at: None,
            object: "fine_tuning.job".to_owned(),
            model: request.model.clone(),
            status: FineTuningJobStatus::Succeeded,
            hyperparameters: Value::Null,
            organization_id: String::new(),
            trained_tokens: None,
            training_file: request.training_file.clone(),
            validation_file: None,
            integrations: None,
            seed: request.seed,
            estimated_finish: None,
            method: None,
            metadata: None,
            fine_tuned_model: Some(format!("{}@sha256:{digest}", request.model)),
            result_files: vec![format!("sha256:{digest}")],
            error: None,
        })
    }
}

/// The text of the subject a dataset line holds.
pub fn memorised(line: &str) -> String {
    let line: Value = serde_json::from_str(line).unwrap();
    let state: Value =
        serde_json::from_str(line["messages"][1]["content"].as_str().unwrap()).unwrap();
    state["text"].as_str().unwrap().to_owned()
}

/// What a memorising artifact answers.
pub struct Memory(pub Model, pub BTreeSet<String>);

impl ModelClient for Memory {
    fn manifest(&self) -> &Model {
        &self.0
    }

    fn classify(&self, params: &SystemOneRequest) -> std::result::Result<SystemOneResponse, Error> {
        let known = self.1.contains(&params.state.text);
        Ok(answers(params, if known { 1.0 } else { 0.0 }))
    }

    fn train(&self, _: &CreateFineTuningJobRequest) -> std::result::Result<FineTuningJob, Error> {
        Err(Error::Failed("no".to_owned()))
    }
}

pub fn remembered(root: &Path, digest: &str) -> Memory {
    let path = root.join(format!(".lighthouse/models/sha256/{digest}.json"));
    let artifact: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let seen = artifact["seen"]
        .as_array()
        .unwrap()
        .iter()
        .map(|text| text.as_str().unwrap().to_owned())
        .collect();
    Memory(manifest(true), seen)
}

pub fn subject(file: &str, text: &str) -> Subject {
    Subject {
        key: text.to_owned(),
        kind: "file".to_owned(),
        id: text.to_owned(),
        state: State {
            language: "rust".to_owned(),
            file: file.to_owned(),
            text: text.to_owned(),
            facts: Value::Null,
            neighbors: Vec::new(),
        },
    }
}

pub fn label(
    key: &str,
    judgment: Judgment,
    observed: Option<Outcome>,
    subject: Option<Subject>,
) -> Label {
    Label {
        key: key.to_owned(),
        judgment,
        source: Source::Person,
        attribution: json!({"type": "Person", "id": "p"}),
        observed,
        subject,
    }
}

pub fn reviewed(fingerprint: &str, file: &str) -> Diagnostic {
    Diagnostic::new(
        RULE,
        Severity::Warn,
        "m",
        file,
        Span {
            start: Position { line: 1, col: 1 },
            end: Position { line: 1, col: 2 },
        },
        Fingerprint::from_raw(fingerprint),
    )
}

/// A run that left these subjects `(fingerprint, file, text)` for review.
pub fn base(reviews: &[(&str, &str, &str)]) -> Found {
    Found::of(&Run {
        reviews: reviews
            .iter()
            .map(|(f, file, _)| reviewed(f, file))
            .collect(),
        questions: reviews
            .iter()
            .map(|(f, file, text)| ((*f).to_owned(), subject(file, text)))
            .collect(),
        ..Run::default()
    })
}

pub fn policy() -> Evaluation {
    Evaluation {
        precision: 0.9,
        recall: None,
    }
}

pub fn report(labels: &[Label], overlaid: &Overlaid) -> EvalReport {
    let result = grade(RULE, None, None, policy(), labels, &overlaid.found, None);
    EvalReport::pool(vec![result]).unwrap()
}

pub fn over<'a>(
    model: &'a str,
    root: &'a Path,
    decision: &'a DecisionRef,
    models: &'a dyn Fn(&str) -> Result<Box<dyn ModelClient + 'a>>,
) -> Overlay<'a> {
    Overlay {
        model,
        decision,
        root,
        models,
    }
}
