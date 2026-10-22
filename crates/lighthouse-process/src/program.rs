//! What every program Lighthouse runs has in common: one environment, one way
//! to run it and one way to tell what its exit code means. A check, a fix, a
//! verification command and a model provider differ in what they do with the
//! result, not in how the program is started.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{GRACE, Invocation, Lines, MAX_OUTPUT, Policy, Profile, Ran, run, run_lines};

/// What a program inherits from its caller, besides what it declares.
const INHERITED: [&str; 4] = ["PATH", "USER", "LOGNAME", "LANG"];
/// What it inherits unless it has a home of its own.
const PERSONAL: [&str; 2] = ["HOME", "TMPDIR"];
/// The version of the contract between Lighthouse and its programs.
const API_VERSION: &str = "1";
/// How much of stderr an error carries.
const STDERR_SHOWN: usize = 400;

/// Where a program keeps its state.
#[derive(Debug, Clone, Copy)]
pub enum Home<'a> {
    /// The user's `HOME` and `TMPDIR`, which toolchains keep their caches in.
    User,
    /// A directory of the run's own, empty: `HOME` and the XDG base
    /// directories lie inside it, so the program never sees the credentials
    /// or the settings of the user.
    Own(&'a Path),
}

/// The environment of one run of a program, and the secrets in it.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    vars: Vec<(String, String)>,
    /// The variables `${NAME}` was expanded from, with their values.
    secrets: Vec<(String, String)>,
}

impl Environment {
    /// The one environment of every program: `PATH`, `USER`, `LOGNAME` and
    /// `LANG`, the user's `HOME` and `TMPDIR` or a home of the run's own, the
    /// declared variables (a `${NAME}` in a value is the user's variable
    /// `NAME`) and the `context` Lighthouse passes. A variable that is not set
    /// fails, naming the variable and never a value.
    pub fn new(
        declared: &BTreeMap<String, String>,
        context: Vec<(String, String)>,
        home: Home<'_>,
    ) -> Result<Self, String> {
        let user = |names: &[&str]| -> Vec<(String, String)> {
            names
                .iter()
                .filter_map(|k| env::var(k).ok().map(|v| ((*k).to_owned(), v)))
                .collect()
        };
        let mut vars = user(&INHERITED);
        match home {
            Home::User => vars.extend(user(&PERSONAL)),
            Home::Own(dir) => vars.extend(isolated(dir)),
        }
        let mut secrets = Vec::new();
        for (key, value) in declared {
            let (expanded, used) = substitute(value)?;
            secrets.extend(used);
            vars.push((key.clone(), expanded));
        }
        vars.extend(context);
        Ok(Self { vars, secrets })
    }

    /// The variables, in the order they were set; a later one wins.
    pub fn vars(&self) -> &[(String, String)] {
        &self.vars
    }

    /// `text` with the value of every expanded variable written as `${NAME}`,
    /// so that nothing a program echoes leaks a secret into a note or a log.
    pub fn scrub(&self, text: &str) -> String {
        let mut shown = text.to_owned();
        let mut secrets: Vec<&(String, String)> = self.secrets.iter().collect();
        secrets.sort_by_key(|(_, value)| std::cmp::Reverse(value.len()));
        for (name, value) in secrets.into_iter().filter(|(_, value)| !value.is_empty()) {
            shown = shown.replace(value, &format!("${{{name}}}"));
        }
        shown
    }
}

/// One run of a program: the command line with its placeholders filled, where
/// it runs, what it reads, and the limits and the sandbox it lives under.
pub struct Job<'a> {
    pub command: &'a [String],
    pub dir: &'a Path,
    pub stdin: &'a [u8],
    pub environment: &'a Environment,
    pub timeout: Duration,
    /// The project's `[sandbox]` with the program's own lists added.
    pub policy: &'a Policy,
    pub profile: &'a Profile,
}

impl Job<'_> {
    /// Runs the program. A program that cannot start or outlives its timeout
    /// is an error; whatever it exits with is not, see [`Ran::exit`]. What it
    /// printed, on stdout and on stderr, has the secrets of the environment
    /// scrubbed, so nothing it echoes becomes a finding, a note or an error
    /// that holds one.
    pub fn run(&self) -> Result<Ran, String> {
        let mut ran = self.run_keeping_stdout()?;
        ran.stdout = self.scrubbed(&ran.stdout);
        Ok(ran)
    }

    /// Runs the program like [`Job::run`] but leaves stdout as it printed it,
    /// for a program whose stdout is data (the new text of a file) and not
    /// something to read; its stderr is scrubbed.
    pub fn run_keeping_stdout(&self) -> Result<Ran, String> {
        let mut ran = run(&self.invocation())?;
        ran.stderr = self.scrubbed(&ran.stderr);
        Ok(ran)
    }

    /// Runs the program like [`Job::run`], with each line of stdout handed to
    /// `lines` as it is printed instead of kept, as [`run_lines`] does. The
    /// lines are as the program printed them: scrubbing them is up to `lines`,
    /// which has [`Environment::scrub`] for it.
    pub fn run_lines(&self, lines: Lines) -> Result<Ran, String> {
        let mut ran = run_lines(&self.invocation(), lines)?;
        ran.stderr = self.scrubbed(&ran.stderr);
        Ok(ran)
    }

    fn invocation(&self) -> Invocation<'_> {
        Invocation {
            argv: self.command,
            dir: self.dir,
            stdin: self.stdin,
            env: Some(self.environment.vars()),
            limit: self.timeout,
            grace: GRACE,
            max_output: MAX_OUTPUT,
            sandbox: self.policy.sandbox(self.profile),
        }
    }

    /// `bytes` with the secrets scrubbed; untouched when it holds none.
    fn scrubbed(&self, bytes: &[u8]) -> Vec<u8> {
        let text = String::from_utf8_lossy(bytes);
        let clean = self.environment.scrub(&text);
        if clean == text {
            bytes.to_vec()
        } else {
            clean.into_bytes()
        }
    }
}

impl Ran {
    /// The exit code when it is one of `success`; else what happened: a
    /// signal, or a code the program is not expected to exit with, with the
    /// start of its stderr.
    pub fn exit(&self, success: &[i32]) -> Result<i32, String> {
        let stderr = String::from_utf8_lossy(&self.stderr);
        let stderr: String = stderr.trim().chars().take(STDERR_SHOWN).collect();
        match self.code {
            Some(code) if success.contains(&code) => Ok(code),
            Some(code) => Err(format!("exited {code}: {stderr}")),
            None => Err(format!("was ended by a signal: {stderr}")),
        }
    }
}

/// The names of the variables the values of `declared` expand, sorted: what a
/// key stands for, with the hash of the value, instead of the value.
pub fn referenced(declared: &BTreeMap<String, String>) -> BTreeSet<String> {
    declared
        .values()
        .filter_map(|value| variables(value).ok())
        .flatten()
        .map(str::to_owned)
        .collect()
}

/// The names `value` expands: `${NAME}`, with no default and no nesting; any
/// other use of `$` is text. An error says what is wrong with the value.
pub fn variables(value: &str) -> Result<Vec<&str>, String> {
    let mut names = Vec::new();
    let mut rest = value;
    while let Some(at) = rest.find("${") {
        let after = &rest[at + 2..];
        let end = after
            .find('}')
            .ok_or("`${` is not closed: a variable is `${NAME}`")?;
        let name = &after[..end];
        let valid = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(format!(
                "`${{{name}}}` is not a variable: only `${{NAME}}` is expanded, with no default and no nesting"
            ));
        }
        names.push(name);
        rest = &after[end + 1..];
    }
    Ok(names)
}

/// `value` with each `${NAME}` replaced by the user's variable. A variable
/// that is not set is an error that names it.
pub fn expand_value(value: &str) -> Result<String, String> {
    substitute(value).map(|(expanded, _)| expanded)
}

/// What a decision a program serves tells it, as `LIGHTHOUSE_*` variables.
pub fn context(decision: &str, options: &str) -> Vec<(String, String)> {
    vec![
        ("LIGHTHOUSE_DECISION".to_owned(), decision.to_owned()),
        ("LIGHTHOUSE_OPTIONS".to_owned(), options.to_owned()),
        ("LIGHTHOUSE_API_VERSION".to_owned(), API_VERSION.to_owned()),
    ]
}

/// The command line: an argument `fill` gives values for is replaced by them
/// as a whole, so no value can grow into an option, and a value that starts
/// with `-` is refused. A program named by a relative path is the project's
/// own, found from `root`.
pub fn expand(
    command: &[String],
    root: &Path,
    fill: impl Fn(&str) -> Option<Vec<String>>,
) -> Result<Vec<String>, String> {
    let mut expanded = Vec::new();
    for (at, arg) in command.iter().enumerate() {
        if at == 0 && arg.contains('/') && !Path::new(arg).is_absolute() {
            expanded.push(root.join(arg).to_string_lossy().into_owned());
            continue;
        }
        let values = fill(arg).unwrap_or_else(|| vec![arg.clone()]);
        if let Some(bad) = values.iter().find(|v| *v != arg && v.starts_with('-')) {
            return Err(format!(
                "the value `{bad}` of `{arg}` starts with `-`, so it is refused"
            ));
        }
        expanded.extend(values);
    }
    Ok(expanded)
}

/// `value` with each `${NAME}` replaced by the user's variable, and the
/// variables used with their values.
fn substitute(value: &str) -> Result<(String, Vec<(String, String)>), String> {
    let mut out = String::new();
    let mut used = Vec::new();
    let mut rest = value;
    for name in variables(value)? {
        let at = rest.find("${").unwrap_or_default();
        out.push_str(&rest[..at]);
        let found = env::var(name).map_err(|_| format!("the variable `{name}` is not set"))?;
        out.push_str(&found);
        used.push((name.to_owned(), found));
        rest = &rest[at + name.len() + 3..];
    }
    out.push_str(rest);
    Ok((out, used))
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
