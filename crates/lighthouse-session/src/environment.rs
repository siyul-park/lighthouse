//! The personal layers of the configuration: the user's file, which holds for
//! every repository, and the repository-local file. What they say that can run
//! a program or write without asking is part of what trust covers (see
//! [`Personal::trusted`]).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use lighthouse_resource::{Base, read_optional, xdg_dir};
use lighthouse_spec::{Catalog, Layer};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{Result, Session, git::git};

/// The user's file below the config directory of Lighthouse.
const USER_FILE: &str = "config.toml";
/// The repository-local file, relative to the project: what `.gitignore`
/// lists.
pub const LOCAL_PATH: &str = ".lighthouse/local.toml";

/// A personal file that exists: where it is, and what it says.
pub(crate) struct Personal {
    pub(crate) layer: Layer,
}

impl Personal {
    /// What of the file trust covers, canonically: the providers that run a
    /// program, with the variables they are given, and the approval. A budget
    /// or a binding to a provider that is not a program is not. `None` when
    /// the file holds nothing of the kind.
    pub(crate) fn trusted(&self) -> Option<String> {
        let spec = &self.layer.spec;
        let programs: BTreeMap<&String, Value> = spec
            .models
            .providers
            .iter()
            .filter(|(_, provider)| provider.command.is_some())
            .map(|(name, p)| {
                let ran = json!({ "command": p.command, "env": p.env, "keyEnv": p.key_env });
                (name, ran)
            })
            .collect();
        if programs.is_empty() && spec.approval.fix.is_none() {
            return None;
        }
        Some(json!({ "providers": programs, "approval": spec.approval.fix }).to_string())
    }
}

/// An environment key as `config show` prints it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Setting {
    /// The key, as a TOML dotted key.
    pub key: String,
    /// The effective value; the values of a provider's `env` are hidden.
    pub value: Value,
    /// With `--origin`: `bundled`, or the file that set the value, relative
    /// to the project when it lies in it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// The user's file, `$XDG_CONFIG_HOME/lighthouse/config.toml`.
pub fn user_file() -> Option<PathBuf> {
    xdg_dir(Base::Config).map(|dir| dir.join(USER_FILE))
}

/// The repository-local file of the project at `root`.
pub fn local_file(root: &Path) -> PathBuf {
    root.join(LOCAL_PATH)
}

/// The personal files that exist, user first, then local: the later wins.
pub(crate) fn read(root: &Path) -> Result<Vec<Personal>> {
    [user_file(), Some(local_file(root))]
        .into_iter()
        .flatten()
        .filter_map(|path| read_optional(&path).transpose().map(|text| (path, text)))
        .map(|(path, text)| {
            let (origin, text) = (path.display().to_string(), text?);
            if path == local_file(root) && tracked(root, &path) {
                return Err(format!(
                    "{origin}: personal settings are committed: untrack it (git rm --cached) and keep it ignored"
                )
                .into());
            }
            let layer = Layer::parse(&origin, &text)?;
            Ok(Personal { layer })
        })
        .collect()
}

/// Rejects a decision the repository-local file binds that the project does
/// not have: a typo would bind nothing. The user's file holds for every
/// repository, so a decision this one lacks is not an error there.
pub(crate) fn check_local(root: &Path, personal: &[Personal], catalog: &Catalog) -> Result<()> {
    let local = local_file(root).display().to_string();
    let layers = personal.iter().map(|p| &p.layer);
    for layer in layers.filter(|layer| layer.origin == local) {
        if let Some(id) = layer
            .spec
            .rules
            .keys()
            .find(|id| catalog.decisions().all(|d| d.id() != id.as_str()))
        {
            return Err(format!("{local}: `rules.\"{id}\"` names no decision").into());
        }
    }
    Ok(())
}

/// The effective environment of the session's project, key by key; with
/// `origin`, each with the layer it came from.
pub fn show(session: &Session, origin: bool) -> Vec<Setting> {
    let shown = |origin: String| relative(&session.root, origin);
    session
        .layers
        .entries()
        .into_iter()
        .map(|entry| Setting {
            key: entry.key,
            value: entry.value,
            origin: origin.then(|| shown(entry.origin)),
        })
        .collect()
}

/// `origin` relative to the project at `root` when the file lies inside it.
fn relative(root: &Path, origin: String) -> String {
    Path::new(&origin)
        .strip_prefix(root)
        .map_or(origin.clone(), |rest| rest.display().to_string())
}

/// Whether git tracks `path`, in a project that is in a git work tree. Without
/// git, or outside a work tree, nothing is tracked.
fn tracked(root: &Path, path: &Path) -> bool {
    let Some(name) = path.to_str() else {
        return false;
    };
    git(root, &["ls-files", "--error-unmatch", "--", name]).is_ok()
}
