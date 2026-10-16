//! `fake-model`: a model for tests, deterministic and offline. It answers by
//! the source of the subject: `maybe_bad` is surely a violation, `maybe_good`
//! surely not, anything else unsure.
//!
//! Without arguments it is a plugin speaking the protocol (`classify` and
//! `train`, which keeps what it was given in the store it is told). As a
//! `command` provider it has three modes: `command` reads a System One body
//! on stdin and prints the answer, appending a mark to the file `calls` for
//! each run; `weak` answers 0.5 in an envelope, as fenced text; `fail` exits
//! with an error. `command` and `fail` mark the file named by a second
//! argument instead of `calls`, to tell several models apart.

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write, stdin, stdout},
    path::Path,
    process::ExitCode,
};

use lighthouse_model::{canonical, hash::sha256};
use lighthouse_protocol::{
    Answer, Capabilities, CreateFineTuningJobRequest, ErrorObject, FineTuningJob,
    FineTuningJobStatus, Handler, InitializeParams, InitializeResult, Model, NOUL,
    SystemOneRequest, SystemOneResponse, artifact_path, dataset_path, serve,
};
use serde_json::{Value, json};

const ID: &str = "fake-model";

struct Fake;

impl Handler for Fake {
    fn initialize(&mut self, _: InitializeParams) -> Result<InitializeResult, ErrorObject> {
        let model = Model {
            deterministic: true,
            abstains: true,
            ..Model::new(ID, "1", Capabilities::classifier(true))
        };
        Ok(InitializeResult::new(ID, "1", Vec::new(), vec![model]))
    }

    fn classify(&mut self, params: SystemOneRequest) -> Result<SystemOneResponse, ErrorObject> {
        let noul = noul_of(&params.state.text);
        Ok(SystemOneResponse {
            model: params.model,
            answers: answers(params.questions.keys(), noul),
            usage: Value::Null,
        })
    }

    fn train(&mut self, request: CreateFineTuningJobRequest) -> Result<FineTuningJob, ErrorObject> {
        let hex = request
            .training_file
            .strip_prefix("sha256:")
            .ok_or("the training file is not a digest")?
            .to_owned();
        let datasets = request
            .metadata
            .get("datasets")
            .ok_or("no datasets")?
            .clone();
        let dir = request.metadata.get("dir").ok_or("no dir")?.clone();
        let text = fs::read_to_string(dataset_path(Path::new(&datasets), &hex))
            .map_err(|e| format!("the dataset: {e}"))?;
        let lines: Vec<Value> = text
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .map_err(|e| format!("a training line: {e}"))?;
        // An artifact of the store, named by the digest of its canonical form:
        // the same dataset makes the same file.
        let artifact = canonical(&json!({
            "card": {"rows": lines.len(), "chosen": "fake", "metrics": {"brier": 0.25}, "band": {"upper": null, "lower": null}, "trainingFile": request.training_file},
            "subjects": lines,
        }));
        let digest = sha256(&artifact);
        let path = artifact_path(Path::new(&dir), &digest);
        if let Some(store) = path.parent() {
            fs::create_dir_all(store).map_err(|e| e.to_string())?;
        }
        fs::write(path, artifact).map_err(|e| e.to_string())?;
        Ok(FineTuningJob {
            id: "ftjob-1".to_owned(),
            created_at: 0,
            finished_at: None,
            object: "fine_tuning.job".to_owned(),
            model: request.model,
            status: FineTuningJobStatus::Succeeded,
            hyperparameters: Value::Null,
            organization_id: ID.to_owned(),
            trained_tokens: None,
            training_file: request.training_file,
            validation_file: request.validation_file,
            integrations: None,
            seed: request.seed,
            estimated_finish: None,
            method: Some(request.method),
            metadata: Some(request.metadata),
            fine_tuned_model: Some(format!("learned@sha256:{digest}")),
            result_files: vec![format!("sha256:{digest}")],
            error: None,
        })
    }
}

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("command") => command(),
        Some("weak") => weak(),
        Some("fail") => match mark() {
            Ok(()) => ExitCode::from(3),
            Err(_) => ExitCode::FAILURE,
        },
        _ => match serve(stdin().lock(), stdout().lock(), &mut Fake) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("fake-model: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

/// System One as a command: the body on stdin, the answer on stdout.
fn command() -> ExitCode {
    let mut input = String::new();
    if stdin().read_to_string(&mut input).is_err() {
        return ExitCode::FAILURE;
    }
    let Ok(request) = serde_json::from_str::<Value>(&input) else {
        return ExitCode::FAILURE;
    };
    if mark().is_err() {
        return ExitCode::FAILURE;
    }
    let noul = noul_of(request["state"]["text"].as_str().unwrap_or_default());
    let questions = request["questions"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let answers: serde_json::Map<String, Value> = questions
        .keys()
        .map(|id| (id.clone(), json!({"type": NOUL, "noul": noul})))
        .collect();
    println!("{}", json!({"model": request["model"], "answers": answers}));
    ExitCode::SUCCESS
}

/// Appends a mark to the file of calls. Runs are concurrent: an append of one
/// byte is atomic, a read and rewrite is not.
fn mark() -> std::io::Result<()> {
    let name = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "calls".to_owned());
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(name)
        .and_then(|mut file| file.write_all(b"x"))
}

/// A model that is unsure of everything, answering as text in an envelope.
fn weak() -> ExitCode {
    let mut input = String::new();
    let _ = stdin().read_to_string(&mut input);
    println!("{}", json!({"result": "```json\n{\"noul\": 0.5}\n```"}));
    ExitCode::SUCCESS
}

/// P(violation) of a subject by its text.
fn noul_of(text: &str) -> f64 {
    if text.contains("maybe_bad") {
        0.95
    } else if text.contains("maybe_good") {
        0.05
    } else {
        0.5
    }
}

fn answers<'a>(ids: impl Iterator<Item = &'a String>, noul: f64) -> BTreeMap<String, Answer> {
    ids.map(|id| (id.clone(), Answer::new(noul))).collect()
}
