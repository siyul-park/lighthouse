//! The personal layers of the configuration: the user's file, which holds for
//! every repository, and the repository-local file. Their text is read here
//! once, so that what is parsed is what trust is judged on.

use std::path::{Path, PathBuf};

use lighthouse_resource::{Base, read_optional, xdg_dir};
use lighthouse_spec::{Catalog, Layer};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::{Result, Session};

/// The user's file below the config directory of Lighthouse.
const USER_FILE: &str = "config.toml";
/// The repository-local file, relative to the project: what `.gitignore`
/// lists.
pub const LOCAL_PATH: &str = ".lighthouse/local.toml";

/// A personal file that exists: where it is, what it says, and what it means.
pub(crate) struct Personal {
    pub(crate) layer: Layer,
    pub(crate) text: String,
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
            let layer = Layer::parse(&origin, &text)?;
            Ok(Personal { layer, text })
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
    let shown = |origin: String| {
        let root = session.root.display().to_string();
        let inside = origin.strip_prefix(&root);
        inside.map_or(origin.clone(), |rest| {
            rest.trim_start_matches('/').to_owned()
        })
    };
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
