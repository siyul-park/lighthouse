//! Trust in a project, decided by the user and not by the repository: running
//! the programs a project names (its command checks and fixes, its command
//! models, its plugins) needs the user to have said so, for exactly the configuration (the personal files over it
//! too), rules and in-project programs they were shown. A file the repository carries cannot grant it, and a change
//! to any of them withdraws it.
//!
//! The digest is computed from the very bytes the session loaded the
//! configuration and the rules from, so what is trusted is what runs. A program
//! that resolves inside the project is part of the digest by content; a program
//! outside it (`gofmt`, `sh`) is trusted by its command line only. A path inside
//! the project enters the digest relative to the root, so the same inputs in
//! another worktree of the repository give the same digest.
//!
//! Trust is kept per repository (the git common directory, else the root) with
//! one entry per root: a root is trusted when its digest equals the digest of
//! any root trusted under the same key. The `.git` files are read only to find
//! that key; they grant nothing, because the digest must still match.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use lighthouse_protocol::hash::Hasher;

use crate::Result;

/// `1` in this variable trusts every project, for CI.
pub const TRUST_VAR: &str = "LIGHTHOUSE_TRUST";
const FILE: &str = "trust.toml";

/// The roots trusted under one repository (or one plain directory), each with
/// the digest it was trusted at.
type Roots = BTreeMap<String, String>;

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Table {
    #[serde(default)]
    trusted: BTreeMap<String, Entry>,
}

/// An entry of the trust file. Only the current shape (roots under a key)
/// counts; any other, such as the one-digest-per-key shape of an earlier
/// layout, reads as nothing trusted.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum Entry {
    Roots(Roots),
    Unknown(toml::Value),
}

/// What a project asks to be trusted for: a digest of the configuration, the
/// rules and the in-project programs, and the commands in words.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Basis {
    pub(crate) digest: String,
    /// One line per program, as shown to the user.
    pub commands: Vec<String>,
}

/// A program the project would run, with its arguments, for the digest and for the user.
pub(crate) struct Program {
    pub(crate) what: String,
    pub(crate) argv: Vec<String>,
}

impl Basis {
    /// The basis of the project at `root` as loaded from `config`, the
    /// personal files `layers` (origin and text) over it and the rule files
    /// `rules`, running `programs`.
    pub(crate) fn of(
        root: &Path,
        config: &str,
        layers: &[(&str, &str)],
        rules: &BTreeMap<String, String>,
        programs: &[Program],
    ) -> Self {
        let mut hash = Hasher::new();
        field(&mut hash, "config", config.as_bytes());
        for (origin, text) in layers {
            field(&mut hash, "layer-name", origin.as_bytes());
            field(&mut hash, "layer", text.as_bytes());
        }
        for (name, text) in rules {
            field(&mut hash, "rule-name", name.as_bytes());
            field(&mut hash, "rule", text.as_bytes());
        }
        let prefixes = prefixes(root);
        let root = root.canonicalize().ok();
        for command in programs {
            let program = command.argv.first().map_or("", String::as_str);
            field(
                &mut hash,
                "program",
                relative(&prefixes, program).as_bytes(),
            );
            // The program and every argument that names a file inside the
            // project (`sh script.sh`, `python3 x.py`) are trusted by content.
            for (at, arg) in command.argv.iter().enumerate() {
                field(&mut hash, "argument", relative(&prefixes, arg).as_bytes());
                let named = root.as_deref().and_then(|r| {
                    if at == 0 {
                        inside(r, arg)
                    } else {
                        in_project(r, arg)
                    }
                });
                let content = named.and_then(|p| fs::read(p).ok());
                field(&mut hash, "binary", &{
                    let mut file = Hasher::new();
                    file.update(content.unwrap_or_default());
                    file.finish()
                });
            }
        }
        Self {
            digest: hash.hex(),
            commands: programs.iter().map(|c| c.what.clone()).collect(),
        }
    }

    /// Whether the user trusts the project at `root` as this basis describes it.
    pub fn trusted(&self, root: &Path) -> bool {
        if lighthouse_model::var(TRUST_VAR).is_some_and(|v| v == "1") {
            return true;
        }
        if self.digest.is_empty() {
            return false;
        }
        let Ok(known) = read() else {
            return false;
        };
        key(root).is_some_and(|k| {
            known
                .get(&k)
                .is_some_and(|roots| roots.values().any(|d| d == &self.digest))
        })
    }
}

/// Records trust in `root` as `basis` describes it.
pub fn trust(root: &Path, basis: &Basis) -> Result<PathBuf> {
    let mut known = read()?;
    let at = key(root).ok_or("the project root has no usable path")?;
    let name = canonical_name(root).ok_or("the project root has no usable path")?;
    known
        .entry(at)
        .or_default()
        .insert(name, basis.digest.clone());
    write(&known)
}

/// Withdraws trust in `root`; true when there was some.
pub fn revoke(root: &Path) -> Result<bool> {
    let mut known = read()?;
    let mut had = false;
    if let (Some(at), Some(name)) = (key(root), canonical_name(root))
        && let Some(roots) = known.get_mut(&at)
    {
        had = roots.remove(&name).is_some();
        if roots.is_empty() {
            known.remove(&at);
        }
    }
    write(&known)?;
    Ok(had)
}

/// Feeds one field to the digest: a tag and a value, each with its length in
/// front, so no two different lists of fields give the same bytes.
fn field(hash: &mut Hasher, tag: &str, value: &[u8]) {
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

/// `text` with every spelling of the root directory replaced by a marker, so
/// a path inside the project does not depend on where the project is. Only a
/// whole directory is replaced: `/a/wt-other` and `/b/a/wt` stay as they are
/// under `/a/wt`.
fn relative(prefixes: &[String], text: &str) -> String {
    prefixes.iter().fold(text.to_owned(), |text, prefix| {
        let mut out = String::with_capacity(text.len());
        let mut rest = text.as_str();
        while let Some(at) = rest.find(prefix.as_str()) {
            let end = at + prefix.len();
            out.push_str(&rest[..at]);
            let starts = out
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_alphanumeric() || "/._-~".contains(c)));
            let whole = starts && rest[end..].chars().next().is_none_or(|c| c == '/');
            out.push_str(if whole { "{root}" } else { prefix });
            rest = &rest[end..];
        }
        out + rest
    })
}

/// The spellings of `root`, as given and canonical, longest first.
fn prefixes(root: &Path) -> Vec<String> {
    let mut found: Vec<String> = [Some(root.to_path_buf()), root.canonicalize().ok()]
        .into_iter()
        .flatten()
        .map(|p| p.to_string_lossy().trim_end_matches('/').to_owned())
        .filter(|p| !p.is_empty())
        .collect();
    found.sort_by_key(|p| std::cmp::Reverse(p.len()));
    found.dedup();
    found
}

fn canonical_name(root: &Path) -> Option<String> {
    root.canonicalize()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// What trust is kept under: the git common directory when `root` is inside a
/// work tree (every worktree of a repository shares it), else the root.
fn key(root: &Path) -> Option<String> {
    let root = root.canonicalize().ok()?;
    match common_dir(&root) {
        Some(dir) => Some(format!("git:{}", dir.to_string_lossy())),
        None => Some(root.to_string_lossy().into_owned()),
    }
}

/// The common git directory of the work tree holding `root`, read from the
/// files git keeps: `.git` is the directory, or a file `gitdir: <dir>` naming
/// the worktree's directory, whose `commondir` file names the shared one.
fn common_dir(root: &Path) -> Option<PathBuf> {
    let (base, dot_git) = root.ancestors().find_map(|dir| {
        let dot_git = dir.join(".git");
        dot_git.exists().then(|| (dir.to_path_buf(), dot_git))
    })?;
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let text = fs::read_to_string(&dot_git).ok()?;
        let named = text.lines().next()?.strip_prefix("gitdir:")?.trim();
        base.join(named).canonicalize().ok()?
    };
    let common = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(text) => git_dir.join(text.trim()),
        Err(_) => git_dir,
    };
    common.canonicalize().ok().filter(|c| c.is_dir())
}

fn read() -> Result<BTreeMap<String, Roots>> {
    let path = path()?;
    match fs::read_to_string(&path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display()).into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(format!("{}: {e}", path.display()).into()),
    }
}

/// The roots of a trust file; an entry of any other layout is skipped.
fn parse(text: &str) -> std::result::Result<BTreeMap<String, Roots>, toml::de::Error> {
    Ok(toml::from_str::<Table>(text)?
        .trusted
        .into_iter()
        .filter_map(|(key, entry)| match entry {
            Entry::Roots(roots) => Some((key, roots)),
            Entry::Unknown(_) => None,
        })
        .collect())
}

fn write(known: &BTreeMap<String, Roots>) -> Result<PathBuf> {
    let path = path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = toml::to_string(&Table {
        trusted: known
            .iter()
            .map(|(key, roots)| (key.clone(), Entry::Roots(roots.clone())))
            .collect(),
    })?;
    lighthouse_model::write_atomic(&path, &text)?;
    Ok(path)
}

fn path() -> Result<PathBuf> {
    let dir = lighthouse_model::xdg_dir(lighthouse_model::Base::Config)
        .ok_or("no config directory for the trust file (set XDG_CONFIG_HOME or HOME)")?;
    Ok(dir.join(FILE))
}

#[cfg(test)]
mod tests;
