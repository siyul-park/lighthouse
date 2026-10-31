//! The program: how Lighthouse runs any external tool, whatever it is run
//! for. One shape is read at every binding point (a decision's `check` and
//! `fix` of type `command`, the programs of `[fix] verify`, and the `command`
//! of a model provider), and what its `output` says it prints decides what
//! the result is. The contract the program runs under is in
//! [`lighthouse_process`]; the binding point decides which placeholders it
//! accepts and which outputs it may have.

use std::{collections::BTreeMap, fmt, time::Duration as Limit};

use lighthouse_model::Duration;
use lighthouse_process::{Policy, check_host, check_write, variables};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use serde_json::Value;

/// How long a program may run when it names no `timeout`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long an agent and a verification command may run when they name none.
pub const LONG_TIMEOUT: Duration = Duration::from_secs(600);

/// The levels SARIF defines for a result.
const LEVELS: [&str; 4] = ["error", "warning", "note", "none"];
/// Every placeholder a command line may have at some binding point.
const PLACEHOLDERS: [&str; 6] = ["file", "files", "line", "symbol", "decision", "model"];
/// Names that were placeholders once, so that the refusal can say they are gone.
const RETIRED: [&str; 3] = ["rule", "path", "root"];

/// The keys a program table may have, for the loaders that read it beside
/// others.
pub(crate) const FIELDS: [&str; 8] = [
    "command", "env", "timeout", "stdin", "output", "success", "sarif", "sandbox",
];

/// The places a program is bound at, which decide the placeholders it accepts
/// and the outputs it may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// A decision's `check`: findings, per file, per batch or once.
    Check,
    /// A decision's `fix`: changes, per finding.
    Fix,
    /// `[fix] verify`: findings, once per fix run.
    Verify,
    /// The `command` of a model provider that answers (`protocol`).
    Provider,
    /// The `command` of a model provider that changes files, an agent: no
    /// `protocol`.
    Agent,
}

/// How many files a run of a program is given, which its placeholders say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Batching {
    /// `{files}`: as many files per run as the argument list allows.
    Files,
    /// `{file}`: one run per file.
    File,
    /// Neither: one run over the project.
    Once,
}

/// The programs a decision binds at one point: its own, and the ones of the
/// languages that give their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Programs {
    own: Program,
    languages: BTreeMap<String, Program>,
}

impl Programs {
    pub(crate) fn new(own: &Program, languages: BTreeMap<String, Program>) -> Self {
        Self {
            own: own.clone(),
            languages,
        }
    }

    /// The program for the files of `language`: the one the language gives,
    /// else the decision's own; `None` where neither has a command.
    pub fn of(&self, language: &str) -> Option<&Program> {
        self.languages
            .get(language)
            .or_else(|| (!self.own.command.is_empty()).then_some(&self.own))
    }

    /// Whether `language` gives a program of its own.
    pub fn is_language(&self, language: &str) -> bool {
        self.languages.contains_key(language)
    }

    /// The decision's own program, which has no command when every language
    /// gives its own.
    pub fn own(&self) -> &Program {
        &self.own
    }

    /// Every program, the decision's own first when it has a command.
    pub fn all(&self) -> impl Iterator<Item = &Program> {
        (!self.own.command.is_empty())
            .then_some(&self.own)
            .into_iter()
            .chain(self.languages.values())
    }
}

/// A program as the files write it. Where a program is a table of its own
/// (a `check` or a `fix`), its fields stand beside the others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "program")]
pub struct Program {
    /// The program and its arguments, run without a shell. `{file}`,
    /// `{files}`, `{line}`, `{symbol}`, `{decision}` and `{model}` fill whole
    /// arguments where the binding point accepts them; they say how many
    /// files a run is given. A decision's `check` and `fix` may leave it out
    /// when each language it covers gives its own program under
    /// `languages.<id>`.
    #[serde(default)]
    pub command: Vec<String>,
    /// Variables the program is given besides the one allowlist. A value may
    /// hold `${NAME}`, the user's variable `NAME`, which is never written
    /// into a command line, a note, a log or a cache key; a variable that is
    /// not set fails the run, naming it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// How long it may run, such as `30s` or `2m`. Default: `2m`; `10m` for
    /// an agent and for a `[fix] verify` program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
    /// What it reads on stdin: nothing, or the text of `{file}`.
    #[serde(default, skip_serializing_if = "Stdin::is_none")]
    pub stdin: Stdin,
    /// What it prints, which decides the result. Default: `lines` for a
    /// check, `files` for a fix, `status` for a `[fix] verify` program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Output>,
    /// The exit codes that mean the program ran; any other code, a signal or
    /// a timeout is an error. Default: `[0, 1]` for `lines` and `sarif`
    /// (linters exit 1 with findings), `[0]` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<Vec<i32>>,
    /// How a SARIF log is read; only with `output: sarif`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sarif: Option<Sarif>,
    /// What it may reach besides the project's `[sandbox]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<Sandbox>,
}

impl Program {
    /// A program that is only a command line.
    pub fn of(command: Vec<String>) -> Self {
        Self {
            command,
            env: BTreeMap::new(),
            timeout: None,
            stdin: Stdin::None,
            output: None,
            success: None,
            sarif: None,
            sandbox: None,
        }
    }

    /// What it prints, at `point`.
    pub fn output(&self, point: Point) -> Output {
        self.output.unwrap_or(point.outputs().0)
    }

    /// The exit codes that mean it ran, at `point`.
    pub fn success(&self, point: Point) -> Vec<i32> {
        self.success
            .clone()
            .unwrap_or_else(|| match self.output(point) {
                Output::Lines | Output::Sarif => vec![0, 1],
                _ => vec![0],
            })
    }

    /// How long it may run, at `point`.
    pub fn timeout(&self, point: Point) -> Limit {
        let default = match point {
            Point::Verify | Point::Agent => LONG_TIMEOUT,
            Point::Check | Point::Fix | Point::Provider => DEFAULT_TIMEOUT,
        };
        self.timeout.unwrap_or(default).get()
    }

    /// How many files a run is given.
    pub fn batching(&self) -> Batching {
        let uses = |placeholder: &str| self.command.iter().any(|arg| arg == placeholder);
        if uses("{files}") {
            Batching::Files
        } else if uses("{file}") {
            Batching::File
        } else {
            Batching::Once
        }
    }

    /// The command line, shown.
    pub fn shown(&self) -> String {
        self.command.join(" ")
    }

    /// The sandbox policy of the program: the project's with its own lists
    /// added.
    pub fn policy(&self, project: &Policy) -> Policy {
        match &self.sandbox {
            Some(own) => project.with(&own.network, &own.write),
            None => project.clone(),
        }
    }

    /// The first problem with the program bound at `point`, described.
    pub fn problem(&self, point: Point) -> Option<String> {
        let first = self.command.first().map(|p| p.trim());
        if first.is_none_or(str::is_empty) {
            return Some("`command` needs a program".to_owned());
        }
        if first.is_some_and(|p| placeholder(p).is_some() || file_fact(p).is_some()) {
            return Some("the program cannot be a placeholder".to_owned());
        }
        self.command
            .iter()
            .find_map(|arg| refused(arg, point))
            .or_else(|| self.fact_problem(point))
            .or_else(|| self.output_problem(point))
            .or_else(|| self.env_problem())
            .or_else(|| self.sandbox_problem())
    }

    /// A check reads a fact of a file for the files of a run, so the run must
    /// have them: `{file}` or `{files}` must be there too. A fix has the file
    /// of its finding.
    fn fact_problem(&self, point: Point) -> Option<String> {
        if point != Point::Check {
            return None;
        }
        let fact = self.command.iter().find(|arg| file_fact(arg).is_some())?;
        let has_files = self
            .command
            .iter()
            .any(|arg| arg == "{file}" || arg == "{files}");
        (!has_files).then(|| format!("`{fact}` needs `{{file}}` or `{{files}}` in the command"))
    }

    fn output_problem(&self, point: Point) -> Option<String> {
        let (_, allowed) = point.outputs();
        let output = self.output(point);
        if !allowed.contains(&output) {
            let names: Vec<&str> = allowed.iter().map(|o| o.as_str()).collect();
            return Some(format!(
                "a {point} has `output` {}, not `{}`",
                names.join(" or "),
                output.as_str()
            ));
        }
        if self.stdin == Stdin::File && self.batching() == Batching::Files {
            return Some("`stdin: file` needs one file: use `{file}`, not `{files}`".to_owned());
        }
        if self.sarif.is_some() && output != Output::Sarif {
            return Some("`sarif` needs `output: sarif`".to_owned());
        }
        let names_file = self.stdin == Stdin::File || self.command.contains(&"{file}".to_owned());
        if point == Point::Fix && output == Output::Stdout && !names_file {
            return Some(
                "`output: stdout` is the new text of `{file}`: it needs `{file}` or `stdin: file`"
                    .to_owned(),
            );
        }
        let bad_level = self
            .sarif
            .iter()
            .flat_map(|s| &s.levels)
            .find(|l| !LEVELS.contains(&l.as_str()));
        if let Some(level) = bad_level {
            return Some(format!(
                "`sarif.levels` has `{level}`; SARIF levels are error, warning, note and none"
            ));
        }
        if self.success.as_ref().is_some_and(Vec::is_empty) {
            return Some("`success` needs at least one exit code".to_owned());
        }
        None
    }

    fn env_problem(&self) -> Option<String> {
        self.env.iter().find_map(|(key, value)| {
            if key.is_empty() || key.contains('=') {
                return Some(format!(
                    "`env` has the name `{key}`, which is not a variable"
                ));
            }
            if key.starts_with("LIGHTHOUSE_") {
                return Some(format!("`env` cannot set `{key}`: Lighthouse sets it"));
            }
            variables(value)
                .err()
                .map(|why| format!("`env.{key}`: {why}"))
        })
    }

    fn sandbox_problem(&self) -> Option<String> {
        let own = self.sandbox.as_ref()?;
        let hosts = own.network.iter().find_map(|h| {
            check_host(h)
                .err()
                .map(|why| format!("`sandbox.network` entry `{h}`: {why}"))
        });
        hosts.or_else(|| {
            own.write.iter().find_map(|w| {
                check_write(w)
                    .err()
                    .map(|why| format!("`sandbox.write` entry `{w}`: {why}"))
            })
        })
    }
}

impl From<ProgramSpec> for Program {
    fn from(spec: ProgramSpec) -> Self {
        match spec {
            ProgramSpec::Command(command) => Self::of(command),
            ProgramSpec::Program(program) => program,
        }
    }
}

/// What a program is given on stdin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Stdin {
    #[default]
    None,
    /// The text of `{file}`.
    File,
}

impl Stdin {
    fn is_none(&self) -> bool {
        *self == Self::None
    }
}

/// What a program prints, which decides what its result is. A program has one
/// of two results: findings (`lines`, `sarif`, `status`) or changes (`files`,
/// `stdout`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Output {
    /// One finding per line of stdout; a leading `path:line[:col]: ` places it,
    /// and a line that is only a path of the project places it at line 1 of
    /// that file, with the decision's title as the message.
    Lines,
    /// A SARIF 2.1.0 log: one finding per result.
    Sarif,
    /// One finding when the exit code is not in `success`, with the tail of
    /// stderr or stdout as its message.
    Status,
    /// The files the program changed in its scratch copy of the project.
    Files,
    /// The new text of `{file}`, on stdout.
    Stdout,
}

impl Output {
    /// The word a file writes it as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lines => "lines",
            Self::Sarif => "sarif",
            Self::Status => "status",
            Self::Files => "files",
            Self::Stdout => "stdout",
        }
    }

    /// Whether the result is findings, as opposed to changes.
    pub fn finds(self) -> bool {
        matches!(self, Self::Lines | Self::Sarif | Self::Status)
    }
}

/// How a SARIF log is read, under SARIF's own names. A result must pass both
/// lists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sarif {
    /// Globs over the result's `ruleId`, such as `errcheck` or `govet:*`: `*`
    /// is any run of characters inside one segment, segments being separated
    /// by `/` or `::`, and `**` is any number of segments, as in module path
    /// globs. Default: every rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
    /// SARIF `level`s to keep: `error`, `warning`, `note` or `none`; a result
    /// without a level is a `warning`. Default: every level.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<String>,
    /// The unit the program counts columns in, when its log does not say or
    /// says wrongly. Default: the log's `columnKind`, else `utf16CodeUnits`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<SarifColumns>,
}

/// The unit a SARIF log counts columns in (`columnKind`, plus the bytes that
/// tools such as golangci-lint count without saying so).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SarifColumns {
    /// UTF-16 code units.
    Utf16CodeUnits,
    /// Unicode code points.
    UnicodeCodePoints,
    /// Bytes.
    Bytes,
}

/// What a program may reach besides what the project's `[sandbox]` lets every
/// program reach: its lists are added to the project's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sandbox {
    /// Hosts the program may reach, through the proxy of the run: a name, or
    /// `*.suffix` for every host below it, either with `:port`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network: Vec<String>,
    /// Places besides its own that it may write: paths below the project, or
    /// below the home directory with `~/` in front. Never a secret.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub write: Vec<String>,
}

impl Point {
    /// The placeholders a command line may have here.
    pub fn accepts(self) -> &'static [&'static str] {
        match self {
            Self::Check => &["file", "files", "decision"],
            Self::Fix => &["file", "line", "symbol", "decision"],
            Self::Verify => &[],
            Self::Provider | Self::Agent => &["model"],
        }
    }

    /// What the output is when the program names none, and the outputs it
    /// may name.
    fn outputs(self) -> (Output, &'static [Output]) {
        match self {
            Self::Check => (
                Output::Lines,
                &[Output::Lines, Output::Sarif, Output::Status],
            ),
            Self::Fix => (Output::Files, &[Output::Files, Output::Stdout]),
            Self::Verify => (Output::Status, &[Output::Lines, Output::Status]),
            Self::Provider | Self::Agent => (Output::Stdout, &[Output::Stdout]),
        }
    }
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Check => "check",
            Self::Fix => "fix",
            Self::Verify => "`[fix] verify` program",
            Self::Provider => "provider",
            Self::Agent => "agent",
        })
    }
}

/// A program as a list of arguments (a program with only a `command`) or as
/// a table: the form where either is accepted, such as the programs of
/// `[fix] verify`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ProgramSpec {
    Command(Vec<String>),
    Program(Program),
}

impl<'de> Deserialize<'de> for ProgramSpec {
    /// Names what was expected, which an untagged enum's own error does not.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            list @ Value::Array(_) => serde_json::from_value(list)
                .map(Self::Command)
                .map_err(D::Error::custom),
            Value::Object(table) => {
                if let Some(stray) = table.keys().find(|k| !FIELDS.contains(&k.as_str())) {
                    return Err(D::Error::custom(format!(
                        "unknown field `{stray}` for a program"
                    )));
                }
                serde_json::from_value(Value::Object(table))
                    .map(Self::Program)
                    .map_err(D::Error::custom)
            }
            other => Err(D::Error::custom(format!(
                "a program is a list of arguments or a table with a `command`, not {other}"
            ))),
        }
    }
}

impl From<&Program> for ProgramSpec {
    fn from(program: &Program) -> Self {
        ProgramSpec::Program(program.clone())
    }
}

/// The name of the fact of a file that `arg` is the placeholder of, when it
/// is one of its own: `edition` for `{file.edition}`, and nothing for
/// `{file}` or `--edition={file.edition}`.
pub fn file_fact(arg: &str) -> Option<&str> {
    let name = arg.strip_prefix("{file.")?.strip_suffix('}')?;
    let mut chars = name.chars();
    (chars.next()?.is_ascii_lowercase() && chars.all(|c| c.is_ascii_alphanumeric())).then_some(name)
}

/// `arg` as a placeholder, when it is one of its own: `{file}` and not
/// `--file={file}`.
fn placeholder(arg: &str) -> Option<&str> {
    arg.strip_prefix('{')?
        .strip_suffix('}')
        .filter(|name| PLACEHOLDERS.contains(name))
}

/// Why `arg` is not a command-line argument at `point`: it has a placeholder
/// the point does not accept, or mixes one into other text.
fn refused(arg: &str, point: Point) -> Option<String> {
    let accepted = point.accepts();
    if arg.contains("{file.") {
        return match file_fact(arg) {
            None => Some(format!(
                "`{arg}` is not a placeholder of its own: `{{file.<fact>}}` fills a whole argument, and a fact is a name such as `edition`"
            )),
            Some(_) if accepted.contains(&"file") => None,
            Some(_) => Some(format!("`{arg}` is not a placeholder of a {point}")),
        };
    }
    for name in names_in(arg) {
        if !PLACEHOLDERS.contains(&name) && !RETIRED.contains(&name) {
            continue;
        }
        let own = arg == format!("{{{name}}}");
        if accepted.contains(&name) {
            if !own {
                return Some(format!(
                    "`{arg}` is not a placeholder of its own: `{{{name}}}` fills a whole argument"
                ));
            }
        } else {
            let list: Vec<String> = accepted.iter().map(|p| format!("`{{{p}}}`")).collect();
            let list = if list.is_empty() {
                "none".to_owned()
            } else {
                list.join(", ")
            };
            let gone = if RETIRED.contains(&name) {
                " (it is gone)"
            } else {
                ""
            };
            return Some(format!(
                "`{{{name}}}` is not a placeholder of a {point}{gone}; it accepts {list}"
            ));
        }
    }
    None
}

/// The `{name}` words in `text`.
fn names_in(text: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('{') {
        let after = &rest[at + 1..];
        match after.find('}') {
            Some(end) if after[..end].chars().all(|c| c.is_ascii_lowercase()) && end > 0 => {
                names.push(&after[..end]);
                rest = &after[end + 1..];
            }
            _ => rest = after,
        }
    }
    names
}
