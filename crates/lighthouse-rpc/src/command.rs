//! A model that is any program, declared as a `command` provider. With
//! `protocol = "typesafe"` it reads TypeSafe's System One request on stdin and
//! prints the response (classification); with `protocol = "openai"` it reads
//! a chat completion request and prints the response; without a protocol it
//! reads the conversation as text and prints the answer (`chat`), and the same
//! program serves `edit` when it is run in a directory it may change. The
//! program runs under the process contract, so a trusted project is the only
//! one that builds it.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use lighthouse_plugin::{
    Answer, Capabilities, ChatRequest, CreateChatCompletionRequest, CreateChatCompletionResponse,
    EditRequest, Error, Model, ModelClient, Supports, SystemOneRequest, SystemOneResponse,
};
use lighthouse_process::{GRACE, Invocation, MAX_OUTPUT};
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
    env: Vec<(String, String)>,
    timeout: Duration,
    edit_timeout: Duration,
}

impl CommandModel {
    /// The model `model` of a `command` provider, named `name` (the whole
    /// reference), run from `dir`; `None` for a provider of another kind.
    pub fn new(dir: &Path, name: &str, model: &str, provider: &ModelProvider) -> Option<Self> {
        let ModelProvider::Command {
            protocol,
            argv,
            pointer,
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
                capabilities: match protocol {
                    CommandProtocol::TypeSafe => Capabilities::classifier(false),
                    CommandProtocol::OpenAi | CommandProtocol::PlainText => Capabilities {
                        chat: Some(Supports { supported: true }),
                        ..Capabilities::default()
                    },
                },
                version: hash::short(
                    &canonical(&json!({
                        "protocol": format!("{protocol:?}"),
                        "argv": argv,
                        "model": model,
                        "pointer": pointer,
                        "env": env,
                        "keyEnv": key_env,
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
            env: command_env(env, key_env.as_deref()),
            timeout: timeout.map_or(MODEL_TIMEOUT, Limit::get),
            edit_timeout: timeout.map_or(EDIT_TIMEOUT, Limit::get),
        })
    }
}

impl CommandModel {
    /// Runs the program in `dir` for at most `limit` over `input` and returns
    /// what it printed.
    fn run(&self, dir: &Path, limit: Duration, input: &[u8]) -> Result<Vec<u8>, Error> {
        let failed = |reason: String| Error::model(&self.manifest.id, reason);
        let output = lighthouse_process::run(&Invocation {
            argv: &self.argv,
            dir,
            stdin: input,
            env: Some(&self.env),
            limit,
            grace: GRACE,
            max_output: MAX_OUTPUT,
        })
        .map_err(failed)?;
        if !output.success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let shown: String = stderr.chars().take(400).collect();
            return Err(failed(format!("exited with {:?}: {shown}", output.code)));
        }
        Ok(output.stdout)
    }

    /// The completion a program that speaks Chat Completions printed.
    fn completion(&self, stdout: &[u8]) -> Result<CreateChatCompletionResponse, Error> {
        let failed = |reason: String| Error::model(&self.manifest.id, reason);
        let printed = serde_json::from_slice::<Value>(stdout)
            .ok()
            .and_then(|printed| match self.pointer.as_deref() {
                Some(pointer) => printed.pointer(pointer).cloned(),
                None => Some(printed),
            })
            .ok_or_else(|| failed("its output is not a chat completion".to_owned()))?;
        serde_json::from_value(printed)
            .map_err(|e| failed(format!("its output is not a chat completion: {e}")))
    }

    /// The completion of a program that printed its answer as text: all of
    /// stdout, or the string a pointer leads to in a JSON envelope.
    fn text_completion(&self, stdout: &[u8]) -> Result<CreateChatCompletionResponse, Error> {
        let failed = |reason: String| Error::model(&self.manifest.id, reason);
        let text = String::from_utf8_lossy(stdout);
        let answer = match self.pointer.as_deref() {
            Some(pointer) => serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|printed| printed.pointer(pointer)?.as_str().map(str::to_owned))
                .ok_or_else(|| failed(format!("its output has no text at `{pointer}`")))?,
            None => text.trim_end().to_owned(),
        };
        if answer.trim().is_empty() {
            return Err(failed("it printed no answer".to_owned()));
        }
        serde_json::from_value(json!({
            "id": "command",
            "object": "chat.completion",
            "created": 0,
            "model": self.model,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": answer},
                "finish_reason": "stop",
            }],
        }))
        .map_err(|e| failed(e.to_string()))
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
        let stdout = self.run(&self.dir, self.timeout, &input)?;
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

    /// The reply of the program: the completion it printed, or its text
    /// answer as the one choice of a completion.
    fn chat(&self, request: &ChatRequest) -> Result<CreateChatCompletionResponse, Error> {
        let failed = |reason: String| Error::model(&self.manifest.id, reason);
        let mut asked = request.body.clone();
        asked.model.clone_from(&self.model);
        match self.protocol {
            CommandProtocol::TypeSafe => Err(failed("it does not chat".to_owned())),
            CommandProtocol::OpenAi => {
                let input = serde_json::to_vec(&asked).map_err(|e| failed(e.to_string()))?;
                self.completion(&self.run(&self.dir, self.timeout, &input)?)
            }
            CommandProtocol::PlainText => {
                let input = conversation(&asked);
                self.text_completion(&self.run(&self.dir, self.timeout, input.as_bytes())?)
            }
        }
    }

    /// The program run in the directory of the request with the prompt on
    /// stdin; what it printed is kept for the notes, cut to a few kilobytes.
    fn edit(&self, request: &EditRequest<'_>) -> Result<String, Error> {
        if self.protocol != CommandProtocol::PlainText {
            return Err(Error::model(&self.manifest.id, "it does not edit"));
        }
        let stdout = self.run(request.dir, self.edit_timeout, request.prompt.as_bytes())?;
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

/// The conversation as plain text: the text of each message in turn, a blank
/// line between them.
fn conversation(request: &CreateChatCompletionRequest) -> String {
    let messages = serde_json::to_value(&request.messages).unwrap_or_default();
    let text = |message: &Value| match message.get("content")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(|part| part.get("text")?.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    };
    let texts: Vec<String> = messages
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(text)
        .collect();
    texts.join("\n\n")
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
    let mut env: Vec<(String, String)> = ["PATH", "HOME", "USER", "LOGNAME", "LANG", "TMPDIR"]
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
