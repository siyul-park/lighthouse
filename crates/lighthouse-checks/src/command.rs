//! The `command` check: an external program under the process contract. It
//! runs without a shell in the project root, bounded in time and output, with
//! a cleared environment. Exit `0` ran and found nothing; `1` ran and printed
//! one finding per stdout line; anything else (`>= 2`, a signal, a timeout, a
//! program that cannot start) is an error that leaves the analysis incomplete,
//! never clean. stderr is for people and only ever explains an error.

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use lighthouse_model::RunScope;
use lighthouse_model::{Diagnostic, Fingerprint, Position, Project, Span};
use lighthouse_plugin::{Ctx, Error as PluginError, RuleManifest};
use lighthouse_process::Spec;
use lighthouse_spec::{Batch, Check, CheckStdin, CommandCheck};
use serde_json::{Map, Value, json};

/// How long a command that was told to stop may take before it is killed.
const GRACE: Duration = Duration::from_secs(2);
/// The version of the contract between Lighthouse and its commands.
const API_VERSION: &str = "1";
/// What a command inherits from its caller, besides what it declares.
const INHERITED: [&str; 3] = ["PATH", "LANG", "TMPDIR"];
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
            timeout: check
                .timeout_duration()
                .unwrap_or(lighthouse_spec::DEFAULT_TIMEOUT),
        }
    }

    pub(crate) fn check(
        &self,
        meta: &RuleManifest,
        ctx: &Ctx,
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
                    let text = self.spec.stdin == CheckStdin::File;
                    let content = text.then(|| read(ctx, &file.path)).transpose()?;
                    found.extend(run.one(&file.path, content.as_deref())?);
                }
                Ok(found)
            }
            (RunScope::Project, Batch::All) => run.all(),
        }
    }
}

struct Run<'a> {
    rule: &'a CommandRule,
    meta: &'a RuleManifest,
    ctx: &'a Ctx<'a>,
    options: &'a Map<String, Value>,
}

impl Run<'_> {
    /// One invocation for one file.
    fn one(&self, file: &Path, text: Option<&str>) -> Result<Vec<Diagnostic>, PluginError> {
        let argv = self.arguments(std::slice::from_ref(&file.to_path_buf()))?;
        let stdin = match self.rule.spec.stdin {
            CheckStdin::None => Vec::new(),
            CheckStdin::File => text.unwrap_or_default().as_bytes().to_vec(),
        };
        let lines = self.invoke(&argv, &stdin)?;
        Ok(self.findings(&lines, Some(file)))
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
            let lines = self.invoke(&argv, &[])?;
            found.extend(self.findings(&lines, None));
        }
        Ok(found)
    }

    /// The command line: `{file}`, `{files}` and `{rule}` fill whole arguments,
    /// and a value that starts with `-` is refused so none can grow into an
    /// option. A program named by a relative path is the project's own.
    fn arguments(&self, files: &[PathBuf]) -> Result<Vec<String>, PluginError> {
        let root = &self.ctx.ws.root;
        let mut argv = Vec::new();
        for (at, arg) in self.rule.spec.argv.iter().enumerate() {
            if at == 0 && arg.contains('/') && !Path::new(arg).is_absolute() {
                argv.push(root.join(arg).to_string_lossy().into_owned());
                continue;
            }
            let values: Vec<String> = match arg.as_str() {
                "{file}" | "{files}" => files.iter().map(|f| slashed(f)).collect(),
                "{rule}" => vec![self.meta.id.clone()],
                other => vec![other.to_owned()],
            };
            if let Some(bad) = values.iter().find(|v| *v != arg && v.starts_with('-')) {
                return Err(PluginError::Incomplete(format!(
                    "{}: the value `{bad}` of `{arg}` starts with `-`, so it is refused",
                    self.meta.id
                )));
            }
            argv.extend(values);
        }
        Ok(argv)
    }

    /// Runs the command; the lines it printed when it found something.
    fn invoke(&self, argv: &[String], stdin: &[u8]) -> Result<Vec<String>, PluginError> {
        let shown = argv.join(" ");
        let incomplete = |what: String| PluginError::Incomplete(format!("`{shown}` {what}"));
        let output = lighthouse_process::run(&Spec {
            argv,
            dir: &self.ctx.ws.root,
            stdin,
            env: Some(&self.environment()),
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
        if codes.clean.contains(&code) {
            return Ok(Vec::new());
        }
        if !codes.findings.contains(&code) {
            return Err(incomplete(format!("exited {code}: {stderr}")));
        }
        if output.truncated {
            return Err(incomplete(
                "printed more than the output cap, so its findings are not all here".to_owned(),
            ));
        }
        let stdout = String::from_utf8(output.stdout)
            .map_err(|_| incomplete("printed output that is not UTF-8".to_owned()))?;
        let lines: Vec<String> = stdout
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .map(str::to_owned)
            .collect();
        if lines.is_empty() {
            return Err(incomplete(format!(
                "exited {code}, which says it found something, but printed no finding: {stderr}"
            )));
        }
        Ok(lines)
    }

    /// The environment a command gets and nothing else: the inherited
    /// allowlist, the declared variables and the context Lighthouse passes.
    fn environment(&self) -> Vec<(String, String)> {
        let mut vars: Vec<(String, String)> = INHERITED
            .iter()
            .filter_map(|k| env::var(k).ok().map(|v| ((*k).to_owned(), v)))
            .collect();
        vars.extend(
            self.rule
                .spec
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        vars.push((
            "LIGHTHOUSE_DECISION".to_owned(),
            json!({ "id": self.meta.id }).to_string(),
        ));
        vars.push((
            "LIGHTHOUSE_OPTIONS".to_owned(),
            Value::Object(self.options.clone()).to_string(),
        ));
        vars.push(("LIGHTHOUSE_API_VERSION".to_owned(), API_VERSION.to_owned()));
        vars
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
                let snippet: String = message.split_whitespace().collect::<Vec<_>>().join(" ");
                Diagnostic::new(
                    &self.meta.id,
                    self.meta.severity,
                    message,
                    &path,
                    Span { start: at, end: at },
                    // Where the line is does not identify the finding: moving
                    // code must not make it a new one. The engine tells
                    // repeats of the same text apart.
                    Fingerprint::of(&self.meta.id, &slashed(&path), &snippet),
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
            .strip_prefix(&self.ctx.ws.root)
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

fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// The text of a file as the run sees it: the overlay, else the disk.
fn read(ctx: &Ctx, path: &Path) -> Result<String, PluginError> {
    if let Some(text) = ctx.ws.overlays.get(path) {
        return Ok(text.clone());
    }
    fs::read_to_string(ctx.ws.root.join(path))
        .map_err(|e| PluginError::Incomplete(format!("{}: {e}", path.display())))
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
