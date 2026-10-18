//! A model that is any program, declared as a `command` provider. With
//! `protocol = "typesafe"` it reads TypeSafe's System One request on stdin and
//! prints the response (classification); without a protocol it serves `edit`:
//! it reads the prompt as text on stdin and changes the files of the directory
//! it is run in. The program runs under the process contract, so a trusted
//! project is the only one that builds it.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use tempfile::TempDir;

use lighthouse_plugin::{
    Answer, Capabilities, EditRequest, Error, Model, ModelClient, SystemOneRequest,
    SystemOneResponse,
};
use lighthouse_process::{GRACE, Invocation, MAX_OUTPUT, Policy, Profile};
use lighthouse_protocol::{
    canonical, hash,
    prompt::{json_in, noul_of},
};
use lighthouse_resource::Duration as Limit;
use lighthouse_spec::{CommandProtocol, ModelProvider};
use serde_json::{Value, json};

/// How long a command model may take when the instance says nothing.
const MODEL_TIMEOUT: Duration = Duration::from_secs(120);
/// How long an agent that edits may take when the instance says nothing.
const EDIT_TIMEOUT: Duration = Duration::from_secs(600);
/// How much of what an agent said is kept.
const NOTES_BYTES: usize = 2048;
/// A program run as a model.
pub struct CommandModel {
    manifest: Model,
    model: String,
    protocol: CommandProtocol,
    argv: Vec<String>,
    pointer: Option<String>,
    dir: PathBuf,
    declared: BTreeMap<String, String>,
    key_env: Option<String>,
    /// How the program runs: the project's `enabled`, and the `network` of
    /// the provider.
    policy: Policy,
    timeout: Duration,
    edit_timeout: Duration,
}

impl CommandModel {
    /// The model `model` of a `command` provider, named `name` (the whole
    /// reference), run from `dir` in the sandbox the project's `sandbox`
    /// says; `None` for a provider of another kind.
    pub fn new(
        dir: &Path,
        name: &str,
        model: &str,
        provider: &ModelProvider,
        sandbox: &Policy,
    ) -> Option<Self> {
        let ModelProvider::Command {
            protocol,
            argv,
            pointer,
            cost,
            timeout,
            env,
            key_env,
            network,
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
                capabilities: match protocol {
                    CommandProtocol::TypeSafe => Capabilities::classifier(false),
                    CommandProtocol::PlainText => Capabilities::default(),
                },
                version: hash::short(
                    &canonical(&json!({
                        "protocol": format!("{protocol:?}"),
                        "argv": argv,
                        "model": model,
                        "pointer": pointer,
                        "env": env,
                        "keyEnv": key_env,
                        "network": network,
                        "program": program_hash(dir, argv),
                    })),
                    6,
                ),
                deterministic: false,
                abstains: true,
                cost: *cost,
            },
            protocol: *protocol,
            argv: argv.iter().map(|a| a.replace("{model}", model)).collect(),
            pointer: pointer.clone(),
            dir: dir.to_owned(),
            declared: env.clone(),
            key_env: key_env.clone(),
            policy: Policy {
                enabled: sandbox.enabled,
                network: network.clone(),
            },
            timeout: timeout.map_or(MODEL_TIMEOUT, Limit::get),
            edit_timeout: timeout.map_or(EDIT_TIMEOUT, Limit::get),
        })
    }
}

impl CommandModel {
    /// Runs the program in `dir` for at most `limit` over `input` and returns
    /// what it printed. Sandboxed, it keeps its own state in a home directory
    /// of the run, and changes the files of `dir` only when it `edits`.
    fn run(
        &self,
        dir: &Path,
        limit: Duration,
        input: &[u8],
        edits: bool,
    ) -> Result<Vec<u8>, Error> {
        let failed = |reason: String| Error::model(&self.manifest.id, reason);
        let home = self
            .policy
            .enabled
            .then(tempfile::tempdir)
            .transpose()
            .map_err(|e| failed(e.to_string()))?;
        let env = command_env(&self.declared, self.key_env.as_deref(), home.as_ref());
        let mut profile = Profile {
            network: self.policy.network.clone(),
            ..Profile::new(&self.dir)
        };
        profile
            .writes
            .extend(home.iter().map(|h| h.path().to_owned()));
        if edits {
            profile.writes.push(dir.to_owned());
        }
        let output = lighthouse_process::run(&Invocation {
            argv: &self.argv,
            dir,
            stdin: input,
            env: Some(&env),
            limit,
            grace: GRACE,
            max_output: MAX_OUTPUT,
            sandbox: self.policy.sandbox(&profile),
        })
        .map_err(failed)?;
        if !output.success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let shown: String = stderr.chars().take(400).collect();
            return Err(failed(format!("exited with {:?}: {shown}", output.code)));
        }
        Ok(output.stdout)
    }
}

impl ModelClient for CommandModel {
    fn manifest(&self) -> &Model {
        &self.manifest
    }

    /// Output that does not fit the shape of an answer is a provider error.
    fn classify(&self, params: &SystemOneRequest) -> Result<SystemOneResponse, Error> {
        let failed = |reason: String| Error::model(&self.manifest.id, reason);
        if self.protocol != CommandProtocol::TypeSafe {
            return Err(failed("it does not classify".to_owned()));
        }
        let mut asked = params.clone();
        asked.model.clone_from(&self.model);
        let input = serde_json::to_vec(&asked).map_err(|e| failed(e.to_string()))?;
        let stdout = self.run(&self.dir, self.timeout, &input, false)?;
        let printed = printed_answer(&stdout, self.pointer.as_deref());
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

    /// The program run in the directory of the request with the prompt on
    /// stdin; what it printed is kept for the notes, cut to a few kilobytes.
    fn edit(&self, request: &EditRequest<'_>) -> Result<String, Error> {
        if self.protocol != CommandProtocol::PlainText {
            return Err(Error::model(&self.manifest.id, "it does not edit"));
        }
        let stdout = self.run(
            request.dir,
            self.edit_timeout,
            request.prompt.as_bytes(),
            true,
        )?;
        Ok(capped(String::from_utf8_lossy(&stdout).trim(), NOTES_BYTES))
    }
}

/// `text` cut to at most `max` bytes, at a character boundary.
fn capped(text: &str, max: usize) -> String {
    let end = (0..=max.min(text.len()))
        .rev()
        .find(|&at| text.is_char_boundary(at))
        .unwrap_or_default();
    text[..end].to_owned()
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
/// the key variable and the declared `env`. In the sandbox `HOME` and the XDG
/// directories lie inside `home`, a directory of the run, unless `env` sets
/// them; without it the program inherits `HOME` and `TMPDIR`.
fn command_env(
    declared: &BTreeMap<String, String>,
    key_env: Option<&str>,
    home: Option<&TempDir>,
) -> Vec<(String, String)> {
    let inherited: &[&str] = if home.is_some() {
        &["PATH", "USER", "LOGNAME", "LANG"]
    } else {
        &["PATH", "HOME", "USER", "LOGNAME", "LANG", "TMPDIR"]
    };
    let mut env: Vec<(String, String)> = inherited
        .iter()
        .copied()
        .chain(key_env)
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_owned(), v)))
        .collect();
    if let Some(home) = home {
        env.extend(isolated(home.path()));
    }
    env.extend(declared.iter().map(|(k, v)| (k.clone(), v.clone())));
    env
}

/// `HOME` and the XDG base directories of a program that keeps its state
/// inside `home`.
fn isolated(home: &Path) -> Vec<(String, String)> {
    let at = |path: PathBuf| path.to_string_lossy().into_owned();
    vec![
        ("HOME".to_owned(), at(home.to_owned())),
        ("XDG_CONFIG_HOME".to_owned(), at(home.join(".config"))),
        ("XDG_DATA_HOME".to_owned(), at(home.join(".local/share"))),
        ("XDG_STATE_HOME".to_owned(), at(home.join(".local/state"))),
        ("XDG_CACHE_HOME".to_owned(), at(home.join(".cache"))),
    ]
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
