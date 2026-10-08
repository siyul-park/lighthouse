//! Trust in a project, decided by the user and not by the repository: running
//! the commands a project names (command fixers, formatters) needs the user to
//! have said so, for exactly the configuration, rules and in-project programs
//! they were shown. A file the repository carries cannot grant it, and a change
//! to any of them withdraws it.
//!
//! The digest is computed from the very bytes the session loaded the
//! configuration and the rules from, so what is trusted is what runs. A program
//! that resolves inside the project is part of the digest by content; a program
//! outside it (`gofmt`, `rustfmt`) is trusted by its command line only.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::Result;

/// `1` in this variable trusts every project, for CI.
pub const TRUST_VAR: &str = "LIGHTHOUSE_TRUST";
/// Where user-level state lives, instead of `~/.lighthouse`.
const HOME_VAR: &str = "LIGHTHOUSE_HOME";
const FILE: &str = "trust.toml";

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Table {
    #[serde(default)]
    trusted: BTreeMap<String, String>,
}

/// What a project asks to be trusted for: a digest of the configuration, the
/// rules and the in-project programs, and the commands in words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Basis {
    pub(crate) digest: String,
    /// One line per formatter and command fixer, as shown to the user.
    pub commands: Vec<String>,
}

/// A command the project would run, for the digest and for the user.
pub(crate) struct Command {
    pub(crate) what: String,
    pub(crate) argv: Vec<String>,
}

impl Basis {
    /// The basis of the project at `root` as loaded from `config` and the rule
    /// files `rules`, running `commands`.
    pub(crate) fn of(
        root: &Path,
        config: &str,
        rules: &BTreeMap<String, String>,
        commands: &[Command],
    ) -> Self {
        let mut hash = Sha256::new();
        field(&mut hash, "config", config.as_bytes());
        for (name, text) in rules {
            field(&mut hash, "rule-name", name.as_bytes());
            field(&mut hash, "rule", text.as_bytes());
        }
        let root = root.canonicalize().ok();
        for command in commands {
            let program = command.argv.first().map_or("", String::as_str);
            field(&mut hash, "program", program.as_bytes());
            // The program and every argument that names a file inside the
            // project (`sh script.sh`, `python3 x.py`) are trusted by content.
            for (at, arg) in command.argv.iter().enumerate() {
                field(&mut hash, "argument", arg.as_bytes());
                let named = root.as_deref().and_then(|r| {
                    if at == 0 {
                        inside(r, arg)
                    } else {
                        in_project(r, arg)
                    }
                });
                let content = named.and_then(|p| fs::read(p).ok());
                field(
                    &mut hash,
                    "binary",
                    &Sha256::digest(content.unwrap_or_default()),
                );
            }
        }
        Self {
            digest: hash.finalize().iter().map(|b| format!("{b:02x}")).collect(),
            commands: commands.iter().map(|c| c.what.clone()).collect(),
        }
    }

    /// Whether the user trusts the project at `root` as this basis describes it.
    pub fn trusted(&self, root: &Path) -> bool {
        if env::var(TRUST_VAR).is_ok_and(|v| v == "1") {
            return true;
        }
        if self.digest.is_empty() {
            return false;
        }
        let Ok(known) = read() else {
            return false;
        };
        key(root).is_some_and(|k| known.get(&k) == Some(&self.digest))
    }
}

/// Records trust in `root` as `basis` describes it.
pub fn trust(root: &Path, basis: &Basis) -> Result<PathBuf> {
    let mut known = read()?;
    known.insert(
        key(root).ok_or("the project root has no usable path")?,
        basis.digest.clone(),
    );
    write(&known)
}

/// Withdraws trust in `root`; true when there was some.
pub fn revoke(root: &Path) -> Result<bool> {
    let mut known = read()?;
    let had = key(root).is_some_and(|k| known.remove(&k).is_some());
    write(&known)?;
    Ok(had)
}

/// Feeds one field to the digest: a tag and a value, each with its length in
/// front, so no two different lists of fields give the same bytes.
fn field(hash: &mut Sha256, tag: &str, value: &[u8]) {
    for part in [tag.as_bytes(), value] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part);
    }
}

/// The file `program` names when it lies inside `root`: a path relative to it
/// or absolute, or a bare name found in a `PATH` directory inside it.
fn inside(root: &Path, program: &str) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = if program.contains('/') {
        vec![root.join(program)]
    } else {
        let path = env::var_os("PATH")?;
        env::split_paths(&path)
            .map(|dir| dir.join(program))
            .collect()
    };
    candidates
        .into_iter()
        .filter_map(|c| c.canonicalize().ok())
        .find(|c| c.starts_with(root) && c.is_file())
}

/// The file `arg` names when it is a path inside `root`, relative to it or
/// absolute.
fn in_project(root: &Path, arg: &str) -> Option<PathBuf> {
    if arg.starts_with('-') || arg.starts_with('{') {
        return None;
    }
    root.join(arg)
        .canonicalize()
        .ok()
        .filter(|p| p.starts_with(root) && p.is_file())
}

fn key(root: &Path) -> Option<String> {
    root.canonicalize()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

fn read() -> Result<BTreeMap<String, String>> {
    let path = path()?;
    match fs::read_to_string(&path) {
        Ok(text) => Ok(toml::from_str::<Table>(&text)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .trusted),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("{}: {e}", path.display()).into()),
    }
}

fn write(known: &BTreeMap<String, String>) -> Result<PathBuf> {
    let path = path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = toml::to_string(&Table {
        trusted: known.clone(),
    })?;
    lighthouse_spec::write_atomic(&path, &text)?;
    Ok(path)
}

fn path() -> Result<PathBuf> {
    let dir = match env::var_os(HOME_VAR) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env::var_os("HOME").ok_or("no home directory for the trust file")?)
            .join(".lighthouse"),
    };
    Ok(dir.join(FILE))
}
