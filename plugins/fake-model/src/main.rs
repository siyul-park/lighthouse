//! `fake-model`: a model for tests, deterministic and offline. It answers by
//! the source of the subject: `maybe_bad` is surely a violation, `maybe_good`
//! surely not, anything else unsure.
//!
//! Without arguments it is a plugin speaking the protocol (`classify` and
//! `train`, which keeps what it was given in the directory it is told). As a
//! `command` provider it has three modes: `command` reads a System One body
//! on stdin and prints the answer, appending a mark to the file `calls` for
//! each run; `weak` answers 0.5 in an envelope, as fenced text; `fail` exits
//! with an error.

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, stdin, stdout},
    process::ExitCode,
};

use lighthouse_protocol::{
    Answer, ClassifyParams, FineTuningJob, FineTuningJobRequest, Handler, HandlerError,
    IndexParams, IndexResult, InitializeParams, InitializeResult, JobStatus, ModelList,
    ModelManifest, SystemOneResponse, VERSION, serve,
};
use serde_json::{Value, json};

const ID: &str = "fake-model";

struct Fake;

impl Handler for Fake {
    fn initialize(&mut self, params: InitializeParams) -> Result<InitializeResult, HandlerError> {
        if params.protocol_version != VERSION {
            return Err(format!("speaks protocol {VERSION}").into());
        }
        Ok(InitializeResult {
            id: ID.to_owned(),
            version: "1".to_owned(),
            protocol_version: VERSION.to_owned(),
            languages: Vec::new(),
            models: ModelList::of(&[ModelManifest {
                id: ID.to_owned(),
                tasks: vec!["classification".to_owned()],
                version: "1".to_owned(),
                trains: true,
                deterministic: true,
                abstains: true,
                cost: 0.0,
            }]),
        })
    }

    fn index(&mut self, _: IndexParams) -> Result<IndexResult, HandlerError> {
        Err("fake-model indexes no language".into())
    }

    fn classify(&mut self, params: ClassifyParams) -> Result<SystemOneResponse, HandlerError> {
        let noul = noul_of(&params.request.state.text);
        Ok(SystemOneResponse {
            model: params.request.model,
            answers: answers(params.request.questions.keys(), noul),
            usage: Value::Null,
        })
    }

    fn train(&mut self, request: FineTuningJobRequest) -> Result<FineTuningJob, HandlerError> {
        let hex = request
            .training_file
            .strip_prefix("sha256:")
            .ok_or("the training file is not a digest")?;
        let datasets = request.metadata.get("datasets").ok_or("no datasets")?;
        let dir = request.metadata.get("dir").ok_or("no dir")?;
        let text = fs::read_to_string(format!("{datasets}/sha256/{hex}.jsonl"))
            .map_err(|e| format!("the dataset: {e}"))?;
        let lines: Vec<Value> = text
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .map_err(|e| format!("a training line: {e}"))?;
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let write = |name: &str, value: Value| {
            fs::write(format!("{dir}/{name}"), value.to_string()).map_err(|e| e.to_string())
        };
        write("subjects.json", Value::Array(lines.clone()))?;
        write("metrics.json", json!({"n": lines.len()}))?;
        Ok(FineTuningJob {
            id: "ftjob-1".to_owned(),
            object: "fine_tuning.job".to_owned(),
            model: request.model,
            status: JobStatus::Succeeded,
            fine_tuned_model: Some("sha256:abc".to_owned()),
            result_files: vec![format!("{dir}/metrics.json")],
            error: None,
        })
    }
}

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("command") => command(),
        Some("weak") => weak(),
        Some("fail") => ExitCode::from(3),
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
    let mark = fs::read_to_string("calls").unwrap_or_default() + "x";
    if fs::write("calls", mark).is_err() {
        return ExitCode::FAILURE;
    }
    let noul = noul_of(request["state"]["text"].as_str().unwrap_or_default());
    let questions = request["questions"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let answers: serde_json::Map<String, Value> = questions
        .keys()
        .map(|id| (id.clone(), json!({"type": "noul", "noul": noul})))
        .collect();
    println!("{}", json!({"model": request["model"], "answers": answers}));
    ExitCode::SUCCESS
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
    ids.map(|id| {
        (
            id.clone(),
            Answer {
                kind: "noul".to_owned(),
                noul,
            },
        )
    })
    .collect()
}
