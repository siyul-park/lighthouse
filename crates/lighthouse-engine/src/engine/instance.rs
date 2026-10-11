//! A model instance of the project, as the engine asks it: a model of a plugin
//! process with the options that pick it, or any program that reads a request
//! and prints an answer. Both are a [`Classifier`] named by the instance.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use lighthouse_model::hash;
use lighthouse_plugin::{
    Answer, Classifier, ClassifyParams, Error, FineTuningJob, FineTuningJobRequest, ModelManifest,
    Registry, SystemOneResponse,
};
use lighthouse_process::{MAX_OUTPUT, Spec};
use lighthouse_protocol::{json_in, noul_of};
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
                    let options = options.clone();
                    Box::new(Rpc {
                        manifest: rpc_manifest(name, inner.manifest(), model, &options, *cost),
                        name: model.to_owned(),
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
                env,
                key_env,
                ..
            } => Box::new(Command {
                model: model.to_owned(),
                manifest: ModelManifest {
                    id: name.to_owned(),
                    tasks: tasks.clone(),
                    version: hash::short(
                        &format!(
                            "{argv:?}{model}{stdin:?}{pointer:?}{env:?}{key_env:?}{}",
                            self.program_hash(argv)
                        ),
                        6,
                    ),
                    trains: false,
                    deterministic: false,
                    abstains: true,
                    cost: *cost,
                },
                argv: argv.iter().map(|a| a.replace("{model}", model)).collect(),
                stdin: *stdin,
                pointer: pointer.clone(),
                dir: self.root.clone(),
                env: command_env(env, key_env.as_deref()),
                timeout: timeout
                    .as_deref()
                    .and_then(parse_duration)
                    .unwrap_or(DEFAULT_TIMEOUT),
            }),
        }
    }
}

impl Host<'_> {
    /// The content hash of the program a command runs, so that a changed
    /// program is a changed model; empty when it cannot be read (a program
    /// found on the `PATH`).
    fn program_hash(&self, argv: &[String]) -> String {
        let Some(program) = argv.first() else {
            return String::new();
        };
        let path = self.root.join(program);
        let found = if path.is_file() {
            path
        } else {
            std::path::PathBuf::from(program)
        };
        std::fs::read(found).map(hash::sha256).unwrap_or_default()
    }
}

/// An instance that cannot run, kept so that its failure is reported.
struct Refused {
    manifest: ModelManifest,
    reason: String,
}

/// A model of a plugin process, named and configured by the instance.
struct Rpc<'a> {
    manifest: ModelManifest,
    /// The part of the reference after the provider.
    name: String,
    model: &'a dyn Classifier,
    options: Map<String, Value>,
}

impl Classifier for Refused {
    fn manifest(&self) -> &ModelManifest {
        &self.manifest
    }

    fn classify(&self, _: &ClassifyParams) -> Result<SystemOneResponse, Error> {
        Err(Error::Failed(format!(
            "model `{}`: {}",
            self.manifest.id, self.reason
        )))
    }

    fn train(&self, _: &FineTuningJobRequest) -> Result<FineTuningJob, Error> {
        Err(Error::Failed(format!(
            "model `{}`: {}",
            self.manifest.id, self.reason
        )))
    }
}

/// Any program that reads a request on stdin and prints JSON.
struct Command {
    manifest: ModelManifest,
    model: String,
    argv: Vec<String>,
    stdin: ModelInput,
    pointer: Option<String>,
    dir: PathBuf,
    env: Vec<(String, String)>,
    timeout: Duration,
}

impl Classifier for Rpc<'_> {
    fn manifest(&self) -> &ModelManifest {
        &self.manifest
    }

    fn classify(&self, params: &ClassifyParams) -> Result<SystemOneResponse, Error> {
        let mut asked = params.clone();
        asked.request.model.clone_from(&self.name);
        asked.options.clone_from(&self.options);
        self.model.classify(&asked)
    }

    fn train(&self, request: &FineTuningJobRequest) -> Result<FineTuningJob, Error> {
        let mut asked = request.clone();
        asked.model.clone_from(&self.name);
        asked.options.clone_from(&self.options);
        self.model.train(&asked)
    }
}

impl Classifier for Command {
    fn manifest(&self) -> &ModelManifest {
        &self.manifest
    }

    /// Output that does not fit the shape of an answer is a provider error.
    fn classify(&self, params: &ClassifyParams) -> Result<SystemOneResponse, Error> {
        let failed =
            |reason: String| Error::Failed(format!("model `{}`: {reason}", self.manifest.id));
        let mut asked = params.clone();
        asked.request.model.clone_from(&self.model);
        let input = match self.stdin {
            ModelInput::Request => {
                serde_json::to_vec(&asked.request).map_err(|e| failed(e.to_string()))?
            }
            ModelInput::Prompt => lighthouse_protocol::prompt(&asked.request).into_bytes(),
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
            .request
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

    fn train(&self, _: &FineTuningJobRequest) -> Result<FineTuningJob, Error> {
        Err(Error::Failed(format!(
            "model `{}`: a command model does not train",
            self.manifest.id
        )))
    }
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

fn rpc_manifest(
    name: &str,
    inner: &ModelManifest,
    model: &str,
    options: &Map<String, Value>,
    cost: Option<f64>,
) -> ModelManifest {
    let version = format!(
        "{}+{}",
        inner.version,
        hash::short(&format!("{model}{}", Value::Object(options.clone())), 4)
    );
    ModelManifest {
        id: name.to_owned(),
        version,
        cost: cost.unwrap_or(inner.cost),
        ..inner.clone()
    }
}

fn refused<'a>(name: &str, reason: String) -> Box<dyn Classifier + 'a> {
    Box::new(Refused {
        manifest: ModelManifest {
            id: name.to_owned(),
            tasks: vec!["classification".to_owned()],
            version: String::new(),
            trains: false,
            deterministic: false,
            abstains: true,
            cost: 0.0,
        },
        reason,
    })
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
