//! A model instance of the project, as the engine asks it: a model of a plugin
//! process with the options that pick it, or any program that reads a request
//! and prints an answer. Both are a [`Classifier`] named by the instance.

use std::{path::PathBuf, time::Duration};

use lighthouse_model::hash;
use lighthouse_plugin::{
    Answer, Classifier, ClassifyParams, ClassifyResult, Error, ModelManifest, ModelRef, Registry,
    ResultKind, TrainParams, TrainResult,
};
use lighthouse_process::{MAX_OUTPUT, Spec};
use lighthouse_resource::parse_duration;
use lighthouse_spec::{ModelInput, ModelProvider};
use serde_json::{Map, Value};

/// How long a command model may take when the instance says nothing.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a command that was told to stop may take before it is killed.
const GRACE: Duration = Duration::from_secs(2);

/// What building an instance needs from the run.
pub(super) struct Host<'a> {
    pub(super) registry: &'a Registry,
    pub(super) root: PathBuf,
    /// The project is trusted to run the commands it names.
    pub(super) trusted: bool,
}

impl<'a> Host<'a> {
    /// The model `model` of `provider`, named `name` (the whole reference);
    /// one that cannot be used is a model that fails with the reason, so its
    /// subjects stay for review.
    pub(super) fn build(
        &self,
        name: &str,
        model: &str,
        provider: &ModelProvider,
    ) -> Box<dyn Classifier + 'a> {
        match provider {
            ModelProvider::Rpc {
                plugin,
                options,
                cost,
            } => match self.registry.model(plugin) {
                Some(inner) => {
                    let mut options = options.clone();
                    options.insert("model".to_owned(), Value::from(model));
                    Box::new(Rpc {
                        manifest: rpc_manifest(name, inner.manifest(), &options, *cost),
                        model: inner,
                        options,
                    })
                }
                None => refused(name, format!("plugin `{plugin}` offers no model")),
            },
            ModelProvider::Command { .. } if !self.trusted => refused(
                name,
                "this project is not trusted to run commands (run `lighthouse trust`)".to_owned(),
            ),
            ModelProvider::Command {
                argv,
                stdin,
                pointer,
                tasks,
                cost,
                timeout,
                ..
            } => Box::new(Command {
                manifest: ModelManifest {
                    id: name.to_owned(),
                    tasks: tasks.clone(),
                    version: hash::short(&format!("{argv:?}{model}{stdin:?}{pointer:?}"), 6),
                    methods: vec!["classify".to_owned()],
                    deterministic: false,
                    abstains: true,
                    cost: *cost,
                },
                argv: argv.iter().map(|a| a.replace("{model}", model)).collect(),
                stdin: *stdin,
                pointer: pointer.clone(),
                dir: self.root.clone(),
                timeout: timeout
                    .as_deref()
                    .and_then(parse_duration)
                    .unwrap_or(DEFAULT_TIMEOUT),
            }),
        }
    }
}

fn rpc_manifest(
    name: &str,
    inner: &ModelManifest,
    options: &Map<String, Value>,
    cost: Option<f64>,
) -> ModelManifest {
    let version = format!(
        "{}+{}",
        inner.version,
        hash::short(&Value::Object(options.clone()).to_string(), 4)
    );
    ModelManifest {
        id: name.to_owned(),
        version,
        cost: cost.unwrap_or(inner.cost),
        ..inner.clone()
    }
}

/// An instance that cannot run, kept so that its failure is reported.
struct Refused {
    manifest: ModelManifest,
    reason: String,
}

fn refused<'a>(name: &str, reason: String) -> Box<dyn Classifier + 'a> {
    Box::new(Refused {
        manifest: ModelManifest {
            id: name.to_owned(),
            tasks: vec!["classification".to_owned()],
            version: String::new(),
            methods: Vec::new(),
            deterministic: false,
            abstains: true,
            cost: 0.0,
        },
        reason,
    })
}

impl Classifier for Refused {
    fn manifest(&self) -> &ModelManifest {
        &self.manifest
    }

    fn classify(&self, _: &ClassifyParams) -> Result<ClassifyResult, Error> {
        Err(Error::Failed(format!(
            "model `{}`: {}",
            self.manifest.id, self.reason
        )))
    }

    fn train(&self, _: &TrainParams) -> Result<TrainResult, Error> {
        Err(Error::Failed(format!(
            "model `{}`: {}",
            self.manifest.id, self.reason
        )))
    }
}

/// A model of a plugin process, named and configured by the instance.
struct Rpc<'a> {
    manifest: ModelManifest,
    model: &'a dyn Classifier,
    options: Map<String, Value>,
}

impl Classifier for Rpc<'_> {
    fn manifest(&self) -> &ModelManifest {
        &self.manifest
    }

    fn classify(&self, params: &ClassifyParams) -> Result<ClassifyResult, Error> {
        let mut asked = params.clone();
        asked.options.clone_from(&self.options);
        let mut result = self.model.classify(&asked)?;
        result.model.id.clone_from(&self.manifest.id);
        Ok(result)
    }

    fn train(&self, params: &TrainParams) -> Result<TrainResult, Error> {
        let mut asked = params.clone();
        asked.options.clone_from(&self.options);
        self.model.train(&asked)
    }
}

/// Any program that reads a request on stdin and prints JSON.
struct Command {
    manifest: ModelManifest,
    argv: Vec<String>,
    stdin: ModelInput,
    pointer: Option<String>,
    dir: PathBuf,
    timeout: Duration,
}

impl Classifier for Command {
    fn manifest(&self) -> &ModelManifest {
        &self.manifest
    }

    /// Whatever the program prints that does not fit the shape of an answer is
    /// an abstention for the subject it concerns.
    fn classify(&self, params: &ClassifyParams) -> Result<ClassifyResult, Error> {
        let failed =
            |reason: String| Error::Failed(format!("model `{}`: {reason}", self.manifest.id));
        let input = match self.stdin {
            ModelInput::Request => serde_json::to_vec(params).map_err(|e| failed(e.to_string()))?,
            ModelInput::Prompt => lighthouse_protocol::prompt(params).into_bytes(),
        };
        let output = lighthouse_process::run(&Spec {
            argv: &self.argv,
            dir: &self.dir,
            stdin: &input,
            env: None,
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
        let given = answers(&output.stdout, self.pointer.as_deref());
        let results = params
            .subjects
            .iter()
            .map(|subject| {
                given
                    .iter()
                    .find(|a| a.key == subject.key)
                    .cloned()
                    .unwrap_or_else(|| abstain(&subject.key))
            })
            .collect();
        Ok(ClassifyResult {
            results,
            model: ModelRef {
                id: self.manifest.id.clone(),
                version: self.manifest.version.clone(),
            },
        })
    }

    fn train(&self, _: &TrainParams) -> Result<TrainResult, Error> {
        Err(Error::Failed(format!(
            "model `{}`: a command model does not train",
            self.manifest.id
        )))
    }
}

fn abstain(key: &str) -> Answer {
    Answer {
        key: key.to_owned(),
        kind: ResultKind::Review,
        confidence: 0.0,
        reason: "the model's answer did not fit the expected shape".to_owned(),
        span: None,
    }
}

/// The answers in what a program printed: JSON, or text holding JSON, at the
/// pointer if there is one; either `{"results": [...]}` or the array itself.
/// Entries that do not fit are left out.
fn answers(stdout: &[u8], pointer: Option<&str>) -> Vec<Answer> {
    let Ok(printed) = serde_json::from_slice::<Value>(stdout) else {
        return Vec::new();
    };
    let inner = match pointer {
        Some(pointer) => printed.pointer(pointer).cloned(),
        None => Some(printed),
    };
    let inner = match inner {
        Some(Value::String(text)) => json_in(&text),
        other => other,
    };
    let list = match inner {
        Some(Value::Array(list)) => list,
        Some(Value::Object(mut map)) => match map.remove("results") {
            Some(Value::Array(list)) => list,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    list.into_iter()
        .filter_map(|entry| serde_json::from_value::<Answer>(entry).ok())
        .map(|mut answer| {
            answer.confidence = answer.confidence.clamp(0.0, 1.0);
            answer
        })
        .collect()
}

/// The JSON in a model's text, which may be wrapped in a fenced block.
fn json_in(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map_or(trimmed, |rest| rest.trim_end_matches("```").trim());
    serde_json::from_str(unfenced).ok()
}
