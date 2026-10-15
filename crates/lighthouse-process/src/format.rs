//! The command that formats a file of a language, run the same way for the
//! check that compares a file with its formatted text and for the fix that
//! formats what it edited.

use std::{collections::BTreeMap, fs, path::Path, time::Duration};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{MAX_OUTPUT, Spec, run};

/// Settings files of formatters that look for them upward from the file.
const SETTINGS: [&str; 4] = [
    "rustfmt.toml",
    ".rustfmt.toml",
    ".editorconfig",
    "rust-toolchain.toml",
];

/// How long a formatter may run on one file.
const LIMIT: Duration = Duration::from_secs(60);
/// How long a formatter that was told to stop may take to do so.
const GRACE: Duration = Duration::from_secs(2);

/// What a formatter is given on stdin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FormatterStdin {
    #[default]
    None,
    /// The text of the file.
    File,
}

/// Where a formatter leaves the formatted text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FormatterOutput {
    /// In the scratch copy of the file, which is appended to `argv` (or fills
    /// an `{file}` argument).
    #[default]
    InPlace,
    /// On stdout, which carries only the text.
    Text,
}

/// The command that formats a file of a language, by the command contract of
/// fixes: exit `0` succeeds, stdout carries only the result, stderr is for
/// people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Formatter {
    pub argv: Vec<String>,
    #[serde(default)]
    pub stdin: FormatterStdin,
    #[serde(default)]
    pub output: FormatterOutput,
    /// Extra environment variables; the rest is cleared to an allowlist.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl Formatter {
    /// Runs the formatter on `text` of `file`, a project-relative path, and
    /// returns the formatted text. It runs with `root` as its working
    /// directory, so version managers resolve and a stdin formatter finds the
    /// project's settings from there; `{path}` is the file's real path, for
    /// tools that take one. A formatter that works on a file gets a scratch
    /// copy of it, `{file}`, next to copies of the settings files found between
    /// it and the root. A failure, a timeout and output that overflowed are
    /// errors, never text.
    pub fn format(&self, root: &Path, file: &Path, text: &str) -> Result<String, String> {
        let scratch = tempfile::tempdir().map_err(|e| e.to_string())?;
        let copy = scratch.path().join(file);
        if let Some(dir) = copy.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        fs::write(&copy, text).map_err(|e| e.to_string())?;
        copy_settings(root, file, scratch.path());
        let path = copy.to_string_lossy().into_owned();
        let real = file.to_string_lossy().replace('\\', "/");
        let mut command: Vec<String> = self
            .argv
            .iter()
            .map(|a| match a.as_str() {
                "{file}" => path.clone(),
                "{path}" => real.clone(),
                other => other.to_owned(),
            })
            .collect();
        let named = self.argv.iter().any(|a| a == "{file}");
        if self.stdin == FormatterStdin::None && !named {
            command.push(path);
        }
        let stdin = match self.stdin {
            FormatterStdin::None => Vec::new(),
            FormatterStdin::File => text.as_bytes().to_vec(),
        };
        let mut env: Vec<(String, String)> = ["PATH", "HOME", "LANG", "TMPDIR"]
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_owned(), v)))
            .collect();
        env.extend(self.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        let name = &self.argv[0];
        let output = run(&Spec {
            argv: &command,
            dir: root,
            stdin: &stdin,
            env: Some(&env),
            limit: LIMIT,
            grace: GRACE,
            max_output: MAX_OUTPUT,
        })
        .map_err(|e| format!("formatter `{name}`: {e}"))?;
        if !output.success {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "formatter `{name}` failed: {}",
                stderr.lines().next().unwrap_or("no message")
            ));
        }
        if output.truncated {
            return Err(format!("formatter `{name}` printed more than it may"));
        }
        match self.output {
            FormatterOutput::InPlace => {
                fs::read_to_string(&copy).map_err(|e| format!("formatter output: {e}"))
            }
            FormatterOutput::Text => match String::from_utf8(output.stdout) {
                Ok(out) if out.is_empty() => Err("the formatter printed no text".to_owned()),
                Ok(out) => Ok(out),
                Err(_) => Err("the formatter printed text that is not UTF-8".to_owned()),
            },
        }
    }
}

/// Copies the formatter settings files that apply to `file`, found by walking
/// up from its directory to `root`, to the same places in `scratch`, so a tool
/// that looks for them upward from the file finds them.
fn copy_settings(root: &Path, file: &Path, scratch: &Path) {
    let mut dir = file.parent();
    while let Some(rel) = dir {
        for name in SETTINGS {
            let from = root.join(rel).join(name);
            if from.is_file() {
                let to = scratch.join(rel).join(name);
                if let Some(parent) = to.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::copy(&from, &to);
            }
        }
        dir = rel.parent().filter(|p| p != &rel);
        if rel.as_os_str().is_empty() {
            break;
        }
    }
}
