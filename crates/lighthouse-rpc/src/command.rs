//! A model that is any program, declared as a `command` provider. With
//! `protocol = "typesafe"` it reads TypeSafe's System One request on stdin and
//! prints the response (classification); without a protocol it serves `agent`:
//! it reads the prompt as text on stdin and changes the files of the directory
//! it is run in. The program runs under the process contract of every
//! program, so a trusted project is the only one that builds it.

use std::path::{Path, PathBuf};

use lighthouse_plugin::{
    Answer, Capabilities, EditRequest, Error, Model, ModelClient, SystemOneRequest,
    SystemOneResponse,
};
use lighthouse_process::{Environment, Home, Job, Policy, referenced};
use lighthouse_protocol::{
    canonical, hash,
    prompt::{json_in, noul_of},
};
use lighthouse_spec::{CommandProtocol, ModelProvider, Point, Program};
use serde_json::{Value, json};

/// How much of what an agent said is kept.
const NOTES_BYTES: usize = 2048;
/// A program run as a model.
pub struct CommandModel {
    manifest: Model,
    model: String,
    protocol: CommandProtocol,
    /// The program, with `{model}` filled in.
    program: Program,
    pointer: Option<String>,
    dir: PathBuf,
    /// How the program runs: the project's `[sandbox]` with the lists of the
    /// provider's own added.
    policy: Policy,
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
            program,
            pointer,
            cost,
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
                        "program": program,
                        "model": model,
                        "pointer": pointer,
                        "secrets": secrets(program),
                        "binary": program_hash(dir, &program.command),
                    })),
                    6,
                ),
                deterministic: false,
                abstains: true,
                cost: *cost,
            },
            protocol: *protocol,
            program: Program {
                command: program
                    .command
                    .iter()
                    .map(|a| a.replace("{model}", model))
                    .collect(),
                ..program.clone()
            },
            pointer: pointer.clone(),
            dir: dir.to_owned(),
            policy: program.policy(sandbox),
        })
    }
}

impl CommandModel {
    /// Runs the program in `dir` for at most `limit` over `input` and returns
    /// what it printed. Sandboxed, it keeps its own state in a home directory
    /// of the run, and changes the files of `dir` only when it `edits`.
    /// Returns what it printed, and the environment it ran in, which can tell
    /// a value of a secret from the text around it.
    fn run(&self, dir: &Path, input: &[u8], edits: bool) -> Result<(Vec<u8>, Environment), Error> {
        let failed = |reason: String| Error::model(&self.manifest.id, reason);
        let home = self
            .policy
            .enabled
            .then(tempfile::tempdir)
            .transpose()
            .map_err(|e| failed(e.to_string()))?;
        let environment = Environment::new(
            &self.program.env,
            Vec::new(),
            home.as_ref().map_or(Home::User, |h| Home::Own(h.path())),
        )
        .map_err(failed)?;
        let mut profile = self.policy.profile(&self.dir, dir);
        profile
            .writes
            .extend(home.iter().map(|h| h.path().to_owned()));
        if edits {
            profile.writes.push(dir.to_owned());
        }
        let ran = Job {
            command: &self.program.command,
            dir,
            stdin: input,
            environment: &environment,
            timeout: self.program.timeout(self.point()),
            policy: &self.policy,
            profile: &profile,
        }
        .run()
        .map_err(failed)?;
        ran.exit(&self.program.success(self.point()))
            .map_err(failed)?;
        Ok((ran.stdout, environment))
    }

    /// Where the program is bound: an agent when it edits.
    fn point(&self) -> Point {
        match self.protocol {
            CommandProtocol::PlainText => Point::Agent,
            CommandProtocol::TypeSafe => Point::Provider,
        }
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
        let (stdout, _) = self.run(&self.dir, &input, false)?;
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
        let (stdout, environment) = self.run(request.dir, request.prompt.as_bytes(), true)?;
        let said = environment.scrub(String::from_utf8_lossy(&stdout).trim());
        Ok(capped(&said, NOTES_BYTES))
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

/// The variables a program's `env` expands, with a hash of the value of each:
/// what the model's version holds of a secret, never the value.
fn secrets(program: &Program) -> Value {
    referenced(&program.env)
        .into_iter()
        .map(|name| {
            let hashed = std::env::var(&name).map(hash::sha256).unwrap_or_default();
            (name, Value::from(hashed))
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
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
