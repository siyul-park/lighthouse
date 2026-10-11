//! A model that is any program: it reads a request on stdin and prints JSON.
//! The project declares it as a `command` provider; the program runs under the
//! process contract, so a trusted project is the only one that builds it.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use lighthouse_model::hash;
use lighthouse_plugin::{
    Answer, Capabilities, Classifier, CreateFineTuningJobRequest, Error, FineTuningJob, Model,
    Supports, SystemOneRequest, SystemOneResponse,
};
use lighthouse_process::{MAX_OUTPUT, Spec};
use lighthouse_protocol::{json_in, noul_of};
use lighthouse_resource::parse_duration;
use lighthouse_spec::{ModelInput, ModelProvider};
use serde_json::Value;

/// How long a command model may take when the instance says nothing.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a command that was told to stop may take before it is killed.
const GRACE: Duration = Duration::from_secs(2);

/// A program run as a model.
pub struct Command {
    manifest: Model,
    model: String,
    argv: Vec<String>,
    stdin: ModelInput,
    pointer: Option<String>,
    dir: PathBuf,
    env: Vec<(String, String)>,
    timeout: Duration,
}

impl Command {
    /// The model `model` of a `command` provider, named `name` (the whole
    /// reference), run from `dir`; `None` for a provider of another kind.
    pub fn new(dir: &Path, name: &str, model: &str, provider: &ModelProvider) -> Option<Self> {
        let ModelProvider::Command {
            argv,
            stdin,
            pointer,
            tasks,
            cost,
            timeout,
            env,
            key_env,
            ..
        } = provider
        else {
            return None;
        };
        Some(Self {
            model: model.to_owned(),
            manifest: Model {
                id: name.to_owned(),
                object: "model".to_owned(),
                created: 0,
                owned_by: name.to_owned(),
                capabilities: Capabilities {
                    classification: tasks
                        .iter()
                        .any(|t| t == "classification")
                        .then_some(Supports { supported: true }),
                    embedding: tasks
                        .iter()
                        .any(|t| t == "embedding")
                        .then_some(Supports { supported: true }),
                    fine_tuning: None,
                },
                version: hash::short(
                    &format!(
                        "{argv:?}{model}{stdin:?}{pointer:?}{env:?}{key_env:?}{}",
                        program_hash(dir, argv)
                    ),
                    6,
                ),
                deterministic: false,
                abstains: true,
                cost: *cost,
            },
            argv: argv.iter().map(|a| a.replace("{model}", model)).collect(),
            stdin: *stdin,
            pointer: pointer.clone(),
            dir: dir.to_owned(),
            env: command_env(env, key_env.as_deref()),
            timeout: timeout
                .as_deref()
                .and_then(parse_duration)
                .unwrap_or(DEFAULT_TIMEOUT),
        })
    }
}

impl Classifier for Command {
    fn manifest(&self) -> &Model {
        &self.manifest
    }

    /// Output that does not fit the shape of an answer is a provider error.
    fn classify(&self, params: &SystemOneRequest) -> Result<SystemOneResponse, Error> {
        let failed =
            |reason: String| Error::Failed(format!("model `{}`: {reason}", self.manifest.id));
        let mut asked = params.clone();
        asked.model.clone_from(&self.model);
        let input = match self.stdin {
            ModelInput::Request => serde_json::to_vec(&asked).map_err(|e| failed(e.to_string()))?,
            ModelInput::Prompt => lighthouse_protocol::prompt(&asked).into_bytes(),
        };
        let output = lighthouse_process::run(&Spec {
            argv: &self.argv,
            dir: &self.dir,
            stdin: &input,
            env: Some(&self.env),
            limit: self.timeout,
            grace: GRACE,
            max_output: MAX_OUTPUT,
        })
        .map_err(failed)?;
        if !output.success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let shown: String = stderr.chars().take(400).collect();
            return Err(failed(format!("exited with {:?}: {shown}", output.code)));
        }
        let printed = printed_answer(&output.stdout, self.pointer.as_deref());
        let answers = params
            .questions
            .keys()
            .map(|id| {
                let noul = printed
                    .as_ref()
                    .and_then(|printed| noul_of(printed, id))
                    .ok_or_else(|| failed(format!("its output has no `noul` for `{id}`")))?;
                let answer = Answer {
                    kind: "noul".to_owned(),
                    noul,
                };
                Ok((id.clone(), answer))
            })
            .collect::<Result<_, Error>>()?;
        Ok(SystemOneResponse {
            model: self.manifest.id.clone(),
            answers,
            usage: Value::Null,
        })
    }

    fn train(&self, _: &CreateFineTuningJobRequest) -> Result<FineTuningJob, Error> {
        Err(Error::Failed(format!(
            "model `{}`: a command model does not train",
            self.manifest.id
        )))
    }
}

/// The content hash of the program a command runs, so that a changed program
/// is a changed model; empty when it cannot be read (a program found on the
/// `PATH`).
fn program_hash(root: &Path, argv: &[String]) -> String {
    let Some(program) = argv.first() else {
        return String::new();
    };
    let path = root.join(program);
    let found = if path.is_file() {
        path
    } else {
        PathBuf::from(program)
    };
    std::fs::read(found).map(hash::sha256).unwrap_or_default()
}

/// The variables a command is given: the allowlist of the command contract,
/// the declared `env` and the key variable.
fn command_env(
    declared: &BTreeMap<String, String>,
    key_env: Option<&str>,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = ["PATH", "HOME", "LANG", "TMPDIR"]
        .into_iter()
        .chain(key_env)
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_owned(), v)))
        .collect();
    env.extend(declared.iter().map(|(k, v)| (k.clone(), v.clone())));
    env
}

/// The answer in what a program printed: JSON, at the pointer if there is
/// one, and text holding JSON when the pointer leads to a string.
fn printed_answer(stdout: &[u8], pointer: Option<&str>) -> Option<Value> {
    let printed = serde_json::from_slice::<Value>(stdout).ok()?;
    let inner = match pointer {
        Some(pointer) => printed.pointer(pointer).cloned()?,
        None => printed,
    };
    match inner {
        Value::String(text) => json_in(&text),
        other => Some(other),
    }
}
