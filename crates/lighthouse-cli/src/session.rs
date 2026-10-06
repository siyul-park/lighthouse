//! What every command that reads a project needs: its configuration, the
//! registry of bundled, project-local and language plugins, and the project's
//! own catalog layer.

use std::{
    env,
    path::{Path, PathBuf},
};

use lighthouse_config::{Config, FILE_NAME};
use lighthouse_declarative::{Declarative, load_local};
use lighthouse_plugin::Registry;
use lighthouse_rpc::Registered;
use lighthouse_spec::Catalog;

use crate::Result;

pub struct Session {
    pub config: Config,
    pub root: PathBuf,
    /// The catalog layer of `.lighthouse/rules`, when the project has one.
    pub local: Option<Catalog>,
}

impl Session {
    /// Loads `config`, or discovers `lighthouse.toml` upward from the current
    /// directory, and the project's local rules.
    pub fn load(config: Option<&Path>) -> Result<Self> {
        Self::open(config, true)
    }

    /// Like `load`, but a missing `lighthouse.toml` means the default
    /// configuration over the current directory.
    pub fn load_or_default(config: Option<&Path>) -> Result<Self> {
        Self::open(config, false)
    }

    fn open(config: Option<&Path>, required: bool) -> Result<Self> {
        let (config, root) = match config {
            Some(path) => (Config::load(path)?, env::current_dir()?),
            None if !required && Config::discover(&env::current_dir()?)?.is_none() => {
                (Config::parse("plugins = [\"core\"]")?, env::current_dir()?)
            }
            None => {
                let (path, config) = Config::discover(&env::current_dir()?)?
                    .ok_or_else(|| format!("no {FILE_NAME} found (run `lighthouse init`)"))?;
                let root = path.parent().ok_or("config path has no parent")?.to_owned();
                (config, root)
            }
        };
        let local = load_local(&root)?;
        Ok(Self {
            config,
            root,
            local,
        })
    }

    /// The bundled and local plugins plus the language plugins the config
    /// lists, which are started here.
    pub fn registry(&self) -> Result<(Registry, Registered)> {
        let mut registry = lighthouse_builtin::registry();
        if let Some(local) = &self.local {
            let plugin = Declarative::from_catalog("local", local)?;
            if !plugin.is_empty() {
                registry.register(&plugin)?;
            }
        }
        let registered = lighthouse_rpc::register(
            &mut registry,
            &self.config,
            &self.root,
            &lighthouse_rpc::search_dirs(&self.root),
        )?;
        Ok((registry, registered))
    }

    /// The bundled catalog with the project's layer on top.
    pub fn catalog(&self) -> Result<Catalog> {
        layered(self.local.as_ref())
    }
}

/// The project root as commands that need no plugins see it: the directory of
/// the `lighthouse.toml` found upward from the current directory, else the
/// current directory. Nothing of the project is loaded, so a broken rule or
/// catalog cannot get in the way.
pub fn project_root() -> Result<PathBuf> {
    let here = env::current_dir()?;
    match Config::discover(&here)? {
        Some((path, _)) => Ok(path.parent().ok_or("config path has no parent")?.to_owned()),
        None => Ok(here),
    }
}

/// The catalog of the project at `root`: the bundled one with the project's
/// layer on top. Fails when the project's layer is broken.
pub fn catalog_at(root: &Path) -> Result<Catalog> {
    layered(load_local(root)?.as_ref())
}

fn layered(local: Option<&Catalog>) -> Result<Catalog> {
    match local {
        Some(local) => Ok(Catalog::overlay(Catalog::bundled(), local)?),
        None => Ok(Catalog::bundled().clone()),
    }
}
