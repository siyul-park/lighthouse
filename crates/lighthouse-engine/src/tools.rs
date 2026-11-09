//! What the findings of a program depend on besides the project's code: the
//! program as the host finds it, its version, and the files that choose the
//! version or configure it. A tool is as it was when its file, its version
//! answer, and those files are. The answer to `--version` is kept in the graph
//! under the file's identity, so that no run starts a program it has asked.

use std::{
    collections::{BTreeSet, HashMap},
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, UNIX_EPOCH},
};

use lighthouse_graph::{Durability, Error, Graph, Input};
use lighthouse_model::ucm::File;
use lighthouse_protocol::hash::Hasher;
use rayon::prelude::*;

/// Files that choose which version of a toolchain runs, or configure a
/// formatter, wherever the project has a directory.
const SELECTORS: [&str; 36] = [
    ".go-version",
    ".tool-versions",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".node-version",
    ".nvmrc",
    ".python-version",
    ".editorconfig",
    "rustfmt.toml",
    ".rustfmt.toml",
    ".prettierrc",
    ".prettierrc.json",
    ".prettierrc.yaml",
    ".prettierrc.yml",
    ".prettierrc.js",
    ".prettierrc.toml",
    "prettier.config.js",
    "prettier.config.mjs",
    "ruff.toml",
    ".ruff.toml",
    "pyproject.toml",
    "package.json",
    // markdownlint-cli2 (its own files, and markdownlint's) and lychee.
    ".markdownlint-cli2.jsonc",
    ".markdownlint-cli2.yaml",
    ".markdownlint-cli2.cjs",
    ".markdownlint-cli2.mjs",
    ".markdownlint.jsonc",
    ".markdownlint.json",
    ".markdownlint.yaml",
    ".markdownlint.yml",
    ".markdownlint.cjs",
    ".markdownlint.mjs",
    ".markdownlintignore",
    "lychee.toml",
    ".lycheerc.toml",
    ".lycheeignore",
];
/// Files of the user's toolchain managers that choose a version for every
/// project, relative to the home directory.
const GLOBAL_SELECTORS: [&str; 2] = [".goenv/version", ".rustup/settings.toml"];
/// Variables that choose a version.
const SELECTING_VARIABLES: [&str; 3] = ["RUSTUP_TOOLCHAIN", "GOENV_VERSION", "GOROOT"];
/// How long a program may take to say its version.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// What a program said to `--version`, by the file the search path finds and
/// what chooses the version it runs ([`locate`]). Persisted: a run that finds
/// the key as it was starts no program.
static VERSION: Input<String, String> = Input::new("tool_version").persisted();

/// Where a program stands: not on the search path, not readable, or the file
/// the search path finds with the key its answer to `--version` is kept under.
enum Located {
    Missing,
    Unreadable,
    Found {
        path: PathBuf,
        file: String,
        key: String,
    },
}

/// The identity of each program of `tools`, by name, as the host finds it now:
/// the program, its version and what chooses the version or configures it
/// ([`selectors`]). What a program answers to `--version` is asked once for
/// each file the search path finds and what chooses its version, and kept in
/// the graph.
pub(crate) fn identities(
    graph: &mut Graph,
    root: &Path,
    files: &[File],
    tools: &[String],
) -> Result<Vec<(String, String)>, Error> {
    if tools.is_empty() {
        return Ok(Vec::new());
    }
    let selectors = selectors(root, files);
    let mut hasher = Hasher::new();
    for (name, value) in SELECTING_VARIABLES.map(|name| (name, env::var_os(name))) {
        hasher.update(name);
        hasher.update(
            value
                .map(|v| v.as_encoded_bytes().to_vec())
                .unwrap_or_default(),
        );
    }
    hasher.update(&selectors);
    let shared = hasher.hex();
    let located: Vec<Located> = tools.iter().map(|tool| locate(tool, &selectors)).collect();
    let mut asked = Vec::new();
    for (tool, at) in tools.iter().zip(&located) {
        if let Located::Found { key, path, .. } = at
            && graph.input(&VERSION, key)?.is_none()
        {
            asked.push((tool, key, path));
        }
    }
    let answers: Vec<Result<String, String>> = asked
        .par_iter()
        .map(|(tool, _, path)| Ok(probe(path, &["--version"])? + &companion(tool)?))
        .collect();
    let mut said = HashMap::new();
    for ((_, key, _), answer) in asked.into_iter().zip(answers) {
        match answer {
            Ok(version) => graph.set_input(&VERSION, key, version, Durability::Volatile)?,
            Err(trouble) => {
                said.insert(key, trouble);
            }
        }
    }
    let mut out = Vec::with_capacity(tools.len());
    for (tool, at) in tools.iter().zip(&located) {
        let found = match at {
            Located::Missing => format!("{tool}: not found"),
            Located::Unreadable => format!("{tool}: unreadable"),
            Located::Found { key, file, .. } => {
                let version = match said.get(key) {
                    Some(trouble) => trouble.clone(),
                    None => graph.input(&VERSION, key)?.unwrap_or_default().to_string(),
                };
                format!("{file}:{version}")
            }
        };
        let mut hasher = Hasher::new();
        hasher.update(&shared);
        hasher.update(tool);
        hasher.update(found);
        out.push((tool.clone(), hasher.hex()));
    }
    Ok(out)
}

/// What tells the programs of `tools` apart from other ones, without asking
/// them anything: the file the search path finds, the variables that choose
/// a version and the files of the user's toolchain managers. The files of the
/// project that configure them are the project's own.
pub(crate) fn state(tools: &[String]) -> String {
    let mut hasher = Hasher::new();
    for name in SELECTING_VARIABLES {
        hasher.update(name);
        hasher.update(env::var_os(name).unwrap_or_default().as_encoded_bytes());
    }
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        for name in GLOBAL_SELECTORS {
            hasher.update(name);
            hasher.update(fs::read(home.join(name)).unwrap_or_default());
        }
    }
    for tool in tools {
        hasher.update(tool);
        hasher.update(file_of(tool).unwrap_or_default());
    }
    hasher.hex()
}

/// The digest of the files that choose a version or configure a formatter:
/// those of the user's toolchain managers, and those in the directories of
/// the project.
pub(crate) fn selectors(root: &Path, files: &[File]) -> String {
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::from([PathBuf::new()]);
    for file in files {
        dirs.extend(file.path.ancestors().skip(1).map(Path::to_owned));
    }
    let mut hasher = Hasher::new();
    let mut feed = |label: &Path, path: &Path| {
        if let Ok(text) = fs::read(path) {
            hasher.update(label.as_os_str().as_encoded_bytes());
            hasher.update(&text);
        }
    };
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        for name in GLOBAL_SELECTORS {
            feed(Path::new(name), &home.join(name));
        }
    }
    for dir in &dirs {
        for name in SELECTORS {
            let relative = dir.join(name);
            feed(&relative, &root.join(&relative));
        }
    }
    hasher.hex()
}

/// What tells this version of `tool` from another: the file the search path
/// finds, the files and variables that choose the version it runs.
fn locate(tool: &str, selectors: &str) -> Located {
    let Some(path) = which(tool) else {
        return Located::Missing;
    };
    let Some(file) = identity_of(&path) else {
        return Located::Unreadable;
    };
    let mut hasher = Hasher::new();
    hasher.update(&file);
    hasher.update(selectors);
    for variable in SELECTING_VARIABLES {
        hasher.update(variable);
        hasher.update(env::var_os(variable).unwrap_or_default().as_encoded_bytes());
    }
    Located::Found {
        path,
        file,
        key: hasher.hex(),
    }
}

/// The file the search path finds for `tool`, by path, size and time.
fn file_of(tool: &str) -> Option<String> {
    identity_of(&which(tool)?)
}

fn identity_of(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let nanos = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!("{}:{}:{nanos}", path.display(), meta.len()))
}

/// The version of the toolchain a program without a version flag belongs to.
fn companion(tool: &str) -> Result<String, String> {
    match tool {
        "gofmt" => which("go").map_or(Ok(String::new()), |go| probe(&go, &["version"])),
        _ => Ok(String::new()),
    }
}

/// What `program` prints when asked `args`, and how it ended; what says that
/// it did not start or did not answer is the error: a run's trouble, not the
/// program's version.
fn probe(program: &Path, args: &[&str]) -> Result<String, String> {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return Err("does not start".to_owned());
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(2));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("no answer".to_owned());
            }
        }
    }
    child
        .wait_with_output()
        .map_err(|_| "no answer".to_owned())
        .map(|out| {
            format!(
                "{}{}{:?}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
                out.status.code()
            )
        })
}

/// The file the search path finds for `tool`.
fn which(tool: &str) -> Option<PathBuf> {
    let direct = Path::new(tool);
    if direct.components().count() > 1 {
        return direct.canonicalize().ok();
    }
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(tool))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| candidate.canonicalize().ok())
}

#[cfg(test)]
mod tests;
