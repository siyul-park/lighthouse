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

/// What `init` writes and what a project without a config is checked with.
pub const DEFAULT_CONFIG: &str = "plugins = [\"core\"]\nextends = [\"core/recommended\"]\n";

/// A project as the frontends open it: its configuration, its root directory
/// and its local rules layer.
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

    /// The project that `dir` or a directory above it configures with a
    /// `lighthouse.toml`; `None` when there is none, an error when the config
    /// or the local rules are broken. Unlike `load`, it never reads the
    /// process's current directory, so a hook can name the project it was
    /// started for.
    pub fn find_in(dir: &Path) -> Result<Option<Self>> {
        match Config::discover(dir)? {
            Some((path, config)) => {
                let root = path.parent().ok_or("config path has no parent")?.to_owned();
                Ok(Some(Self::assemble(config, root)?))
            }
            None => Ok(None),
        }
    }

    fn open(config: Option<&Path>, required: bool) -> Result<Self> {
        let here = env::current_dir()?;
        if let Some(path) = config {
            return Self::assemble(Config::load(path)?, here);
        }
        match Self::find_in(&here)? {
            Some(session) => Ok(session),
            None if required => Err(format!("no {FILE_NAME} found (run `lighthouse init`)").into()),
            None => Self::assemble(Config::parse(DEFAULT_CONFIG)?, here),
        }
    }

    fn assemble(config: Config, root: PathBuf) -> Result<Self> {
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
        let mut registry = self.in_process_registry()?;
        let registered = lighthouse_rpc::register(
            &mut registry,
            &self.config,
            &self.root,
            &lighthouse_rpc::search_dirs(&self.root),
        )?;
        Ok((registry, registered))
    }

    /// The same project with another local layer, such as a candidate that
    /// has not been written yet.
    pub fn with_local(self, local: Option<Catalog>) -> Self {
        Self { local, ..self }
    }

    /// Plugins that run in this process only: the bundled ones and the local
    /// rules. Nothing is started, so it is cheap and cannot fail on a
    /// language plugin.
    pub fn in_process_registry(&self) -> Result<Registry> {
        let mut registry = lighthouse_builtin::registry();
        if let Some(local) = &self.local {
            let plugin = Declarative::from_catalog("local", local)?;
            if !plugin.is_empty() {
                registry.register(&plugin)?;
            }
        }
        Ok(registry)
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
