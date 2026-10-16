//! The `command` check: an external program under the process contract. It
//! runs without a shell in the project root, bounded in time and output, with
//! a cleared environment. Exit `0` ran and found nothing; `1` ran and printed
//! one finding per stdout line; anything else (`>= 2`, a signal, a timeout, a
//! program that cannot start) is an error that leaves the analysis incomplete,
//! never clean. stderr is for people and only ever explains an error. With
//! `output: sarif` stdout is a SARIF log instead of lines (see [`sarif`]).

pub(crate) mod contract;
pub mod sarif;

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use lighthouse_model::{Diagnostic, Position, Project, RunScope, Span, squash};
use lighthouse_plugin::{Context, Error as PluginError, RuleManifest};
use lighthouse_process::{GRACE, Stdin};
use lighthouse_spec::{Batch, Check, CheckOutput, CommandCheck, DEFAULT_TIMEOUT};
use serde_json::{Map, Value, json};

use crate::glob::slashed;
/// The most bytes of arguments one run is given; more files go to more runs.
const ARGUMENT_BUDGET: usize = 96 * 1024;
/// How much of stderr an error carries.
const STDERR_SHOWN: usize = 400;

/// A `command` check, compiled.
pub(crate) struct CommandRule {
    spec: CommandCheck,
    timeout: Duration,
}

impl CommandRule {
    pub(crate) fn new(check: &Check, spec: &CommandCheck) -> Self {
        Self {
            spec: spec.clone(),
            timeout: check.timeout.unwrap_or(DEFAULT_TIMEOUT).get(),
        }
    }

    pub(crate) fn check(
        &self,
        meta: &RuleManifest,
        ctx: &Context,
        options: &Map<String, Value>,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let incomplete = |reason: String| PluginError::Incomplete(reason);
        if !ctx.trusted {
            return Err(incomplete(format!(
                "`{}` is not run: this project is not trusted to run commands (run `lighthouse trust`, or set LIGHTHOUSE_TRUST=1 in CI)",
                self.spec.argv.join(" ")
            )));
        }
        let run = Run {
            rule: self,
            meta,
            ctx,
            options,
        };
        match (meta.scope, self.spec.batch) {
            (RunScope::File, _) => {
                let Some((file, text)) = ctx.file else {
                    return Ok(Vec::new());
                };
                run.one(&file.path, Some(text))
            }
            (RunScope::Project, Batch::File) => {
                let mut found = Vec::new();
                for file in &ctx.project.files {
                    let text = self.spec.stdin == Stdin::File;
                    let content = text.then(|| ctx.workspace.read(&file.path)).transpose()?;
                    found.extend(run.one(&file.path, content.as_deref())?);
                }
                Ok(found)
            }
            (RunScope::Project, Batch::All) => run.all(),
        }
    }
}

/// What a program that ran printed, and whether its exit code says it found
/// something.
#[derive(Default)]
struct Ran {
    stdout: String,
    /// The start of stderr, which explains a failure.
    stderr: String,
    found: bool,
}

impl Ran {
    /// The non-blank lines of stdout.
    fn lines(&self) -> Vec<String> {
        self.stdout
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned)
            .collect()
    }
}

struct Run<'a> {
    rule: &'a CommandRule,
    meta: &'a RuleManifest,
    ctx: &'a Context<'a>,
    options: &'a Map<String, Value>,
}

impl Run<'_> {
    /// One invocation for one file.
    fn one(&self, file: &Path, text: Option<&str>) -> Result<Vec<Diagnostic>, PluginError> {
        let argv = self.arguments(std::slice::from_ref(&file.to_path_buf()))?;
        let stdin = match self.rule.spec.stdin {
            Stdin::None => Vec::new(),
            Stdin::File => text.unwrap_or_default().as_bytes().to_vec(),
        };
        let ran = self.invoke(&argv, &stdin)?;
        self.collect(&ran, &argv, Some(file))
    }

    /// What the program printed, read as the check says: lines, or SARIF.
    fn collect(
        &self,
        ran: &Ran,
        argv: &[String],
        file: Option<&Path>,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        match self.rule.spec.output {
            CheckOutput::Lines => Ok(self.findings(&ran.lines(), file)),
            CheckOutput::Sarif => {
                let read = sarif::Reading {
                    ctx: self.ctx,
                    meta: self.meta,
                    select: self.rule.spec.select.as_ref(),
                    columns: self.rule.spec.columns,
                    found: ran.found,
                };
                read.diagnostics(&ran.stdout).map_err(|e| {
                    PluginError::Incomplete(format!("`{}` {e}: {}", argv.join(" "), ran.stderr))
                })
            }
        }
    }

    /// Invocations over every file of the project, as many files per run as
    /// the argument list allows.
    fn all(&self) -> Result<Vec<Diagnostic>, PluginError> {
        let files: Vec<PathBuf> = self
            .ctx
            .project
            .files
            .iter()
            .map(|f| f.path.clone())
            .collect();
        let mut found = Vec::new();
        for chunk in chunks(&self.rule.spec.argv, &files) {
            let argv = self.arguments(chunk)?;
            let ran = self.invoke(&argv, &[])?;
            found.extend(self.collect(&ran, &argv, None)?);
        }
        Ok(found)
    }

    /// The command line: `{file}`, `{files}` and `{rule}` fill whole arguments.
    fn arguments(&self, files: &[PathBuf]) -> Result<Vec<String>, PluginError> {
        contract::expand(
            &self.rule.spec.argv,
            &self.ctx.workspace.root,
            |arg| match arg {
                "{file}" | "{files}" => Some(files.iter().map(|f| slashed(f)).collect()),
                "{rule}" => Some(vec![self.meta.id.clone()]),
                _ => None,
            },
        )
        .map_err(|reason| PluginError::Incomplete(format!("{}: {reason}", self.meta.id)))
    }

    /// Runs the command; what it printed when it found something (or, for a
    /// SARIF log, when it ran clean too).
    fn invoke(&self, argv: &[String], stdin: &[u8]) -> Result<Ran, PluginError> {
        let shown = argv.join(" ");
        let incomplete = |what: String| PluginError::Incomplete(format!("`{shown}` {what}"));
        let output = lighthouse_process::run(&lighthouse_process::Invocation {
            argv,
            dir: &self.ctx.workspace.root,
            stdin,
            env: Some(&contract::environment(
                &self.rule.spec.env,
                &json!({ "id": self.meta.id }),
                self.options,
            )),
            limit: self.rule.timeout,
            grace: GRACE,
            max_output: lighthouse_process::MAX_OUTPUT,
        })
        .map_err(incomplete)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr: String = stderr.trim().chars().take(STDERR_SHOWN).collect();
        let Some(code) = output.code else {
            return Err(incomplete(format!("was ended by a signal: {stderr}")));
        };
        let codes = &self.rule.spec.exit_codes;
        let found = codes.findings.contains(&code);
        let clean = codes.clean.contains(&code);
        if !found && !clean {
            return Err(incomplete(format!("exited {code}: {stderr}")));
        }
        if clean && self.rule.spec.output == CheckOutput::Lines {
            return Ok(Ran::default());
        }
        if output.truncated {
            return Err(incomplete(
                "printed more than the output cap, so its findings are not all here".to_owned(),
            ));
        }
        let stdout = String::from_utf8(output.stdout)
            .map_err(|_| incomplete("printed output that is not UTF-8".to_owned()))?;
        let ran = Ran {
            stdout,
            found,
            stderr: stderr.clone(),
        };
        if found && self.rule.spec.output == CheckOutput::Lines && ran.lines().is_empty() {
            return Err(incomplete(format!(
                "exited {code}, which says it found something, but printed no finding: {stderr}"
            )));
        }
        Ok(ran)
    }

    /// One finding per line: a leading `path:line[:col]: ` of a file of the
    /// project places it; any other line attaches to the file the command was
    /// run for, or to the project.
    fn findings(&self, lines: &[String], file: Option<&Path>) -> Vec<Diagnostic> {
        lines
            .iter()
            .map(|line| {
                let (path, position, message) = self.located(line).unwrap_or_else(|| {
                    (
                        file.map_or_else(|| PathBuf::from("."), Path::to_owned),
                        (1, 1),
                        line.trim(),
                    )
                });
                let at = Position {
                    line: position.0,
                    col: position.1,
                };
                let snippet = squash(message);
                // Where the line is does not identify the finding: moving
                // code must not make it a new one. The engine tells
                // repeats of the same text apart.
                let fingerprint = self.meta.fingerprint(&slashed(&path), &snippet);
                Diagnostic::new(
                    &self.meta.id,
                    self.meta.severity,
                    message,
                    &path,
                    Span { start: at, end: at },
                    fingerprint,
                )
            })
            .collect()
    }

    fn located<'l>(&self, line: &'l str) -> Option<(PathBuf, (u32, u32), &'l str)> {
        let (path, rest) = line.split_once(':')?;
        let (number, rest) = rest.split_once(':')?;
        let row: u32 = number.trim().parse().ok().filter(|n| *n > 0)?;
        let (col, message) = match rest.split_once(':') {
            Some((col, message)) => match col.trim().parse::<u32>() {
                Ok(col) if col > 0 => (col, message),
                _ => (1, rest),
            },
            None => (1, rest),
        };
        let path = self.project_path(path)?;
        Some((path, (row, col), message.trim()))
    }

    /// `path` as a file of the project: relative to the root, or absolute
    /// inside it.
    fn project_path(&self, path: &str) -> Option<PathBuf> {
        let candidate = Path::new(path);
        let relative = candidate
            .strip_prefix(&self.ctx.workspace.root)
            .unwrap_or(candidate)
            .components()
            .filter(|c| !matches!(c, std::path::Component::CurDir))
            .collect::<PathBuf>();
        known(self.ctx.project, &relative).then_some(relative)
    }
}

fn known(project: &Project, path: &Path) -> bool {
    project.file(path).is_some()
}

/// Splits `files` into runs whose arguments fit the budget.
fn chunks<'f>(argv: &[String], files: &'f [PathBuf]) -> Vec<&'f [PathBuf]> {
    let fixed: usize = argv.iter().map(|a| a.len() + 1).sum();
    let mut out = Vec::new();
    let mut start = 0;
    let mut used = fixed;
    for (at, file) in files.iter().enumerate() {
        let size = file.as_os_str().len() + 1;
        if at > start && used + size > ARGUMENT_BUDGET {
            out.push(&files[start..at]);
            start = at;
            used = fixed;
        }
        used += size;
    }
    if start < files.len() || files.is_empty() {
        out.push(&files[start..]);
    }
    out
}
