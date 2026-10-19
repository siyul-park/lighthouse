//! The `command` check: a program under the process contract. It runs
//! without a shell in the project root, bounded in time and output, with the
//! one environment. What it printed is read as its `output` says: `lines`
//! (one finding per line), `sarif` (a SARIF log) or `status` (one finding
//! when the exit code is not in `success`). A code outside `success`, a
//! signal, a timeout or a program that cannot start is an error that leaves
//! the analysis incomplete, never clean; stderr is for people and only ever
//! explains an error.

pub mod sarif;

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use lighthouse_model::{Diagnostic, Position, RunScope, Span, squash, ucm::Project};
use lighthouse_plugin::{Context, Error as PluginError, RuleManifest};
use lighthouse_process::{Environment, Home, Job, Ran, context, copy_project, expand};
use lighthouse_protocol::hash::Hasher;
use lighthouse_spec::{Batching, GlobSet, Output, Point, Program, Stdin, glob_set};
use serde_json::{Map, Value, json};
use tempfile::TempDir;

use crate::glob::slashed;

/// The most bytes of arguments one run is given; more files go to more runs.
const ARGUMENT_BUDGET: usize = 96 * 1024;
/// How much of what a failed `status` program printed its finding carries.
const STATUS_SHOWN: usize = 400;

/// A `command` check, compiled.
pub(crate) struct CommandRule {
    program: Program,
    files: Option<GlobSet>,
    /// What the decision says when a line only names a file.
    title: String,
    /// The program belongs to the bundled catalog, so it needs no trust.
    bundled: bool,
    /// The copy of the project with the texts of a fix run in place of its
    /// files, which a program that reads files is run in, and the digest of
    /// the texts it holds.
    copy: Mutex<Option<(String, Arc<TempDir>)>>,
}

impl CommandRule {
    pub(crate) fn new(
        spec: &lighthouse_spec::CommandCheck,
        title: &str,
        bundled: bool,
    ) -> Result<Self, crate::Error> {
        let files = (!spec.files.is_empty())
            .then(|| glob_set(&spec.files))
            .transpose()
            .map_err(|e| crate::Error::invalid(title, e.to_string()))?;
        Ok(Self {
            program: spec.program.clone(),
            files,
            title: title.to_owned(),
            bundled,
            copy: Mutex::new(None),
        })
    }

    pub(crate) fn check(
        &self,
        meta: &RuleManifest,
        ctx: &Context,
        options: &Map<String, Value>,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let incomplete = |reason: String| PluginError::Incomplete(format!("{}: {reason}", meta.id));
        if !ctx.trusted && !self.bundled {
            return Err(PluginError::Incomplete(format!(
                "`{}` is not run: this project is not trusted to run commands (run `lighthouse trust`, or set LIGHTHOUSE_TRUST=1 in CI)",
                self.program.shown()
            )));
        }
        let environment = Environment::new(
            &self.program.env,
            context(
                &json!({ "id": meta.id }).to_string(),
                &Value::Object(options.clone()).to_string(),
            ),
            Home::User,
        )
        .map_err(incomplete)?;
        let held = self.root_for(ctx, ctx.file.map(|(file, _)| file.path.as_path()))?;
        let run = Invocation {
            rule: self,
            meta,
            ctx,
            environment,
            root: held
                .as_ref()
                .map_or_else(|| ctx.workspace.root.clone(), |dir| dir.path().to_owned()),
        };
        match (meta.scope, self.program.batching()) {
            (RunScope::File, _) => {
                let Some((file, text)) = ctx.file else {
                    return Ok(Vec::new());
                };
                if !self.selects(&file.path) {
                    return Ok(Vec::new());
                }
                run.one(&file.path, Some(text))
            }
            (RunScope::Project, Batching::Files) => run.all(),
            (RunScope::Project, Batching::File) => {
                let mut found = Vec::new();
                for file in ctx.project.files.iter().filter(|f| self.selects(&f.path)) {
                    let text = self.program.stdin == Stdin::File;
                    let content = text.then(|| ctx.workspace.read(&file.path)).transpose()?;
                    found.extend(run.one(&file.path, content.as_deref())?);
                }
                Ok(found)
            }
            (RunScope::Project, Batching::Once) => {
                run.collect(&run.invoke(&run.arguments(&[])?, &[])?, None)
            }
        }
    }

    /// Whether the findings for a file are a function of its text: the
    /// program is bundled, so it is known to read nothing else, and it keeps
    /// no secret in its environment.
    pub(crate) fn local(&self, scope: RunScope) -> bool {
        self.bundled && scope == RunScope::File && self.program.env.is_empty()
    }

    /// The copy of the project the program is run in, when the run stands
    /// texts in place of files (a fix run checking candidate edits) and the
    /// program reads files: a program cannot read an overlay from the disk.
    /// One copy serves every run over the same texts.
    fn root_for(
        &self,
        ctx: &Context,
        file: Option<&Path>,
    ) -> Result<Option<Arc<TempDir>>, PluginError> {
        let overlays = &ctx.workspace.overlays;
        let relevant = file.map_or(!overlays.is_empty(), |file| overlays.contains_key(file));
        if !relevant {
            return Ok(None);
        }
        let mut digest = Hasher::new();
        for (path, text) in overlays {
            digest.update(path.to_string_lossy().as_bytes());
            digest.update(text.as_bytes());
        }
        let digest = digest.hex();
        let mut held = self.copy.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((same, dir)) = held.as_ref()
            && *same == digest
        {
            return Ok(Some(dir.clone()));
        }
        let failed = |e: std::io::Error| {
            PluginError::Incomplete(format!("the copy of the project failed: {e}"))
        };
        let dir = TempDir::new().map_err(failed)?;
        copy_project(&ctx.workspace.root, dir.path(), overlays).map_err(failed)?;
        let dir = Arc::new(dir);
        *held = Some((digest, dir.clone()));
        Ok(Some(dir))
    }

    /// Whether the program is run for `path`.
    fn selects(&self, path: &Path) -> bool {
        self.files.as_ref().is_none_or(|files| files.is_match(path))
    }
}

/// What a program that ran printed, and the exit code it ended with.
struct Printed {
    stdout: String,
    /// The start of stderr, which explains a failure.
    stderr: String,
    code: i32,
}

impl Printed {
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

struct Invocation<'a> {
    rule: &'a CommandRule,
    meta: &'a RuleManifest,
    ctx: &'a Context<'a>,
    environment: Environment,
    /// Where the program runs: the project, or its copy with overlays.
    root: PathBuf,
}

impl Invocation<'_> {
    /// One invocation for one file.
    fn one(&self, file: &Path, text: Option<&str>) -> Result<Vec<Diagnostic>, PluginError> {
        let command = self.arguments(std::slice::from_ref(&file.to_path_buf()))?;
        let stdin = match self.rule.program.stdin {
            Stdin::None => Vec::new(),
            Stdin::File => text.unwrap_or_default().as_bytes().to_vec(),
        };
        let printed = self.invoke(&command, &stdin)?;
        self.collect(&printed, Some(file))
    }

    /// Invocations over every file the program is run for, as many files per
    /// run as the argument list allows.
    fn all(&self) -> Result<Vec<Diagnostic>, PluginError> {
        let files: Vec<PathBuf> = self
            .ctx
            .project
            .files
            .iter()
            .filter(|f| self.rule.selects(&f.path))
            .map(|f| f.path.clone())
            .collect();
        let mut found = Vec::new();
        for chunk in chunks(&self.rule.program.command, &files) {
            let command = self.arguments(chunk)?;
            let printed = self.invoke(&command, &[])?;
            found.extend(self.collect(&printed, None)?);
        }
        Ok(found)
    }

    /// The command line: `{file}`, `{files}` and `{decision}` fill whole
    /// arguments.
    fn arguments(&self, files: &[PathBuf]) -> Result<Vec<String>, PluginError> {
        expand(&self.rule.program.command, &self.root, |arg| match arg {
            "{file}" | "{files}" => Some(files.iter().map(|f| slashed(f)).collect()),
            "{decision}" => Some(vec![self.meta.id.clone()]),
            _ => None,
        })
        .map_err(|reason| PluginError::Incomplete(format!("{}: {reason}", self.meta.id)))
    }

    /// Runs the command and reads what it printed as the program says.
    fn invoke(&self, command: &[String], stdin: &[u8]) -> Result<Printed, PluginError> {
        let shown = command.join(" ");
        let incomplete = |what: String| PluginError::Incomplete(format!("`{shown}` {what}"));
        let program = &self.rule.program;
        let root = &self.root;
        let policy = program.policy(&self.ctx.workspace.sandbox);
        let profile = policy.profile(root, root).caching();
        let ran = Job {
            command,
            dir: root,
            stdin,
            environment: &self.environment,
            timeout: program.timeout(Point::Check),
            policy: &policy,
            profile: &profile,
        }
        .run()
        .map_err(incomplete)?;
        let success = program.success(Point::Check);
        let output = program.output(Point::Check);
        // A `status` program fails by its code, which is its finding.
        let code = match (output, ran.code) {
            (Output::Status, Some(code)) => code,
            _ => ran.exit(&success).map_err(incomplete)?,
        };
        if ran.truncated {
            return Err(incomplete(
                "printed more than the output cap, so its findings are not all here".to_owned(),
            ));
        }
        Ok(Printed {
            code,
            stderr: stderr_of(&ran),
            stdout: String::from_utf8(ran.stdout)
                .map_err(|_| incomplete("printed output that is not UTF-8".to_owned()))?,
        })
    }

    /// What the program printed, read as its `output` says.
    fn collect(
        &self,
        printed: &Printed,
        file: Option<&Path>,
    ) -> Result<Vec<Diagnostic>, PluginError> {
        let program = &self.rule.program;
        match program.output(Point::Check) {
            Output::Lines => {
                let lines = printed.lines();
                if lines.is_empty() && printed.code != 0 {
                    return Err(PluginError::Incomplete(format!(
                        "`{}` exited {} and reported nothing: {}",
                        program.shown(),
                        printed.code,
                        printed.stderr
                    )));
                }
                Ok(self.findings(&lines, file))
            }
            Output::Status => Ok(self.failure(printed, file)),
            _ => {
                let read = sarif::Reading {
                    ctx: self.ctx,
                    root: &self.root,
                    meta: self.meta,
                    sarif: program.sarif.as_ref(),
                    found: printed.code != 0,
                };
                read.diagnostics(&printed.stdout).map_err(|e| {
                    PluginError::Incomplete(format!(
                        "`{}` {e}: {}",
                        program.shown(),
                        printed.stderr
                    ))
                })
            }
        }
    }

    /// The one finding of a `status` program whose code is not a success: the
    /// tail of what it printed is its message.
    fn failure(&self, printed: &Printed, file: Option<&Path>) -> Vec<Diagnostic> {
        if self
            .rule
            .program
            .success(Point::Check)
            .contains(&printed.code)
        {
            return Vec::new();
        }
        let said = if printed.stderr.trim().is_empty() {
            printed.stdout.trim()
        } else {
            printed.stderr.trim()
        };
        let skipped = said.chars().count().saturating_sub(STATUS_SHOWN);
        let tail: String = said.chars().skip(skipped).collect();
        let message = format!(
            "`{}` exited {}{}{tail}",
            self.rule.program.shown(),
            printed.code,
            if tail.is_empty() { "" } else { ": " },
        );
        self.finding(
            &message,
            file.map_or_else(|| PathBuf::from("."), Path::to_owned),
            1,
            1,
        )
    }

    /// One finding per line: a leading `path:line[:col]: ` of a file of the
    /// project places it; a line that is only the path of a file of the
    /// project places it at line 1 of that file, with the title of the
    /// decision as its message; any other line attaches to the file the
    /// command was run for, or to the project.
    fn findings(&self, lines: &[String], file: Option<&Path>) -> Vec<Diagnostic> {
        let mut named = BTreeSet::new();
        lines
            .iter()
            .flat_map(|line| {
                let (path, position, message) = match self.located(line) {
                    Some(placed) => placed,
                    None => match self.named(line) {
                        // A file is named once, though a tool that follows
                        // `mod` declarations (rustfmt) lists it once for each
                        // way it reached it.
                        Some((path, ..)) if !named.insert(path.clone()) => return Vec::new(),
                        Some(placed) => placed,
                        None => (
                            file.map_or_else(|| PathBuf::from("."), Path::to_owned),
                            (1, 1),
                            line.trim(),
                        ),
                    },
                };
                self.finding(message, path, position.0, position.1)
            })
            .collect()
    }

    fn finding(&self, message: &str, path: PathBuf, line: u32, col: u32) -> Vec<Diagnostic> {
        let at = Position { line, col };
        let snippet = squash(message);
        // Where the line is does not identify the finding: moving code must
        // not make it a new one. The engine tells repeats of the same text
        // apart.
        let fingerprint = self.meta.fingerprint(&slashed(&path), &snippet);
        vec![Diagnostic::new(
            &self.meta.id,
            self.meta.severity,
            message,
            &path,
            Span { start: at, end: at },
            fingerprint,
        )]
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

    /// A line that is only the path of a file of the project, as `gofmt -l`,
    /// `rustfmt -l` and `prettier --list-different` print.
    fn named(&self, line: &str) -> Option<(PathBuf, (u32, u32), &str)> {
        let path = self.project_path(line.trim())?;
        Some((path, (1, 1), self.rule.title.as_str()))
    }

    /// `path` as a file of the project: relative to the root, or absolute
    /// inside it, by the name the root is given or by the one the file system
    /// resolves it to (rustfmt prints the latter for the files of a `mod`).
    fn project_path(&self, path: &str) -> Option<PathBuf> {
        let candidate = Path::new(path);
        let roots = [&self.root, &self.ctx.workspace.root];
        let named = roots.iter().map(|root| Some((*root).clone()));
        let resolved = roots.iter().map(|root| root.canonicalize().ok());
        let relative = named
            .chain(resolved)
            .flatten()
            .find_map(|root| candidate.strip_prefix(root).ok())
            .unwrap_or(candidate)
            .components()
            .filter(|c| !matches!(c, std::path::Component::CurDir))
            .collect::<PathBuf>();
        known(self.ctx.project, &relative).then_some(relative)
    }
}

/// The start of what the program said on stderr.
fn stderr_of(ran: &Ran) -> String {
    String::from_utf8_lossy(&ran.stderr)
        .trim()
        .chars()
        .take(STATUS_SHOWN)
        .collect()
}

fn known(project: &Project, path: &Path) -> bool {
    project.file(path).is_some()
}

/// Splits `files` into runs whose arguments fit the budget.
fn chunks<'f>(command: &[String], files: &'f [PathBuf]) -> Vec<&'f [PathBuf]> {
    let fixed: usize = command.iter().map(|a| a.len() + 1).sum();
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
    if start < files.len() {
        out.push(&files[start..]);
    }
    out
}
