//! What the findings of a program depend on besides the project's code: the
//! program as the host finds it, its version, and the files that choose the
//! version or configure it. A tool is as it was when its file, its version
//! answer, and those files are. The answer to `--version` is kept in the
//! cache under the file's identity, so that a run in which nothing changed
//! starts no program.

use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, UNIX_EPOCH},
};

use lighthouse_model::ucm::Project;
use lighthouse_protocol::hash::Hasher;

use super::{Cache, KeyBuilder};

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

/// The identity of `tools`, given the digest of the files that choose their
/// version or configure them ([`selectors`]).
pub(crate) fn identity(tools: &[String], selectors: &str, cache: &Cache) -> String {
    let mut hasher = Hasher::new();
    for (name, value) in SELECTING_VARIABLES.map(|name| (name, env::var_os(name))) {
        hasher.update(name);
        hasher.update(
            value
                .map(|v| v.as_encoded_bytes().to_vec())
                .unwrap_or_default(),
        );
    }
    hasher.update(selectors);
    for tool in tools {
        hasher.update(tool);
        hasher.update(found(tool, selectors, cache));
    }
    hasher.hex()
}

/// The digest of the files that choose a version or configure a formatter:
/// those of the user's toolchain managers, and those in the directories of
/// the project.
pub(crate) fn selectors(root: &Path, project: &Project) -> String {
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::from([PathBuf::new()]);
    for file in &project.files {
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
fn found(tool: &str, selectors: &str, cache: &Cache) -> String {
    let Some(path) = which(tool) else {
        return format!("{tool}: not found");
    };
    let Some(file) = fs::metadata(&path).ok().and_then(|meta| {
        let nanos = meta
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(format!("{}:{}:{nanos}", path.display(), meta.len()))
    }) else {
        return format!("{tool}: unreadable");
    };
    let mut key = KeyBuilder::new();
    key.part("tool-version").part(&file).part(selectors);
    for variable in SELECTING_VARIABLES {
        key.part(env::var_os(variable).unwrap_or_default().as_encoded_bytes());
    }
    let key = key.finish();
    if let Some(version) = cache.get::<String>(&key) {
        return format!("{file}:{version}");
    }
    let version = probe(&path, &["--version"]) + &companion(tool);
    cache.put(key, &version);
    format!("{file}:{version}")
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
