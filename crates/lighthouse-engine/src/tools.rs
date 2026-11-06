//! What the findings of a program depend on besides the project's code: the
//! program as the host finds it, its version, and the files that choose the
//! version or configure it. A tool is as it was when its file, its version
//! answer, and those files are. The answer to `--version` is kept for the
//! process under the file's identity, so that a fix run that checks many times
//! starts each program once.

use std::{
    collections::{BTreeSet, HashMap},
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock, PoisonError},
    time::{Duration, Instant, UNIX_EPOCH},
};

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

/// The identity of each program of `tools`, by name, as the host finds it now:
/// the program, its version and what chooses the version or configures it
/// ([`selectors`]). What a program answers to `--version` is asked once for
/// each file the search path finds in a process, side by side.
pub(crate) fn identities(root: &Path, files: &[File], tools: &[String]) -> Vec<(String, String)> {
    if tools.is_empty() {
        return Vec::new();
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
    let found: Vec<String> = tools
        .par_iter()
        .map(|tool| found(tool, &selectors))
        .collect();
    tools
        .iter()
        .zip(found)
        .map(|(tool, found)| {
            let mut hasher = Hasher::new();
            hasher.update(&shared);
            hasher.update(tool);
            hasher.update(found);
            (tool.clone(), hasher.hex())
        })
        .collect()
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
/// finds, and what it and its companion say of their versions.
fn found(tool: &str, selectors: &str) -> String {
    static ANSWERS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let Some(path) = which(tool) else {
        return format!("{tool}: not found");
    };
    let Some(file) = identity_of(&path) else {
        return format!("{tool}: unreadable");
    };
    let mut key = format!("{file}\0{selectors}");
    for variable in SELECTING_VARIABLES {
        key.push('\0');
        key.push_str(&env::var_os(variable).unwrap_or_default().to_string_lossy());
    }
    let answers = ANSWERS.get_or_init(Mutex::default);
    let known = answers
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned();
    let version = known.unwrap_or_else(|| {
        let version = probe(&path, &["--version"]) + &companion(tool);
        answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, version.clone());
        version
    });
    format!("{file}:{version}")
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
fn companion(tool: &str) -> String {
    match tool {
        "gofmt" => which("go")
            .map(|go| probe(&go, &["version"]))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// What `program` prints when asked `args`, and how it ended.
fn probe(program: &Path, args: &[&str]) -> String {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return "does not start".to_owned();
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
                return "no answer".to_owned();
            }
        }
    }
    child.wait_with_output().map_or_else(
        |_| "no answer".to_owned(),
        |out| {
            format!(
                "{}{}{:?}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
                out.status.code()
            )
        },
    )
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
