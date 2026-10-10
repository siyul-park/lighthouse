//! What every command that reads a project needs: its configuration, the
//! registry of bundled, project-local and language plugins, and the project's
//! own catalog layer.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use lighthouse_checks::Declarative;
use lighthouse_config::{Config, FILE_NAME, Format};
use lighthouse_plugin::Registry;
use lighthouse_rpc::Registered;
use lighthouse_spec::{Catalog, CheckKind, FixKind, load_local, local_files};

use crate::{
    Result,
    trust::{Basis, Command},
};

/// What `init` writes and what a project without a config is checked with.
pub const DEFAULT_CONFIG: &str = "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Project\"\n\n[metadata]\nname = \"project\"\n\n[spec]\nplugins = [\"core\"]\nextends = [\"core/recommended\"]\n";

/// A project as the frontends open it: its configuration, its root directory
/// and its local decisions layer.
pub struct Session {
    pub config: Config,
    pub root: PathBuf,
    /// The catalog layer of `.lighthouse/decisions`, when the project has one.
    pub local: Option<Catalog>,
    /// What the project asks to be trusted for, computed from the same bytes
    /// the configuration and the decisions were loaded from.
    pub basis: Basis,
}

impl Session {
    /// Loads `config`, or discovers `lighthouse.toml` upward from the current
    /// directory, and the project's local decisions.
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
    /// or the local decisions are broken. Unlike `load`, it never reads the
    /// process's current directory, so a hook can name the project it was
    /// started for.
    pub fn find_in(dir: &Path) -> Result<Option<Self>> {
        for ancestor in dir.ancestors() {
            if let Some(path) = Config::file_in(ancestor) {
                let text =
                    fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                return Ok(Some(Self::assemble(&path, &text, ancestor.to_owned())?));
            }
        }
        Ok(None)
    }

    /// Whether the user trusts this project to run the commands it names.
    pub fn trusted(&self) -> bool {
        self.basis.trusted(&self.root)
    }

    fn open(config: Option<&Path>, required: bool) -> Result<Self> {
        let here = env::current_dir()?;
        if let Some(path) = config {
            let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            return Self::assemble(path, &text, here);
        }
        match Self::find_in(&here)? {
            Some(session) => Ok(session),
            None if required => Err(format!("no {FILE_NAME} found (run `lighthouse init`)").into()),
            None => Self::assemble(Path::new(FILE_NAME), DEFAULT_CONFIG, here),
        }
    }

    /// Builds the session from the text of the configuration file at `path`
    /// and the decision files read here, once: what is parsed is what trust is
    /// judged on.
    fn assemble(path: &Path, text: &str, root: PathBuf) -> Result<Self> {
        let format = Format::of_path(path).unwrap_or(Format::Toml);
        let config = Config::parse_as(format, &path.display().to_string(), text)?;
        let files = local_files(&root)?;
        let local = files.clone().map(Catalog::from_local).transpose()?;
        let catalog = layered(local.as_ref())?;
        let commands = commands(&config, &catalog);
        let basis = Basis::of(&root, text, &files.unwrap_or_default(), &commands);
        Ok(Self {
            config,
            root,
            local,
            basis,
        })
    }

    /// A session over `root` with the default configuration and no local
    /// decisions: for commands that must work on a project whose own files
    /// are broken or out of date.
    pub fn bare(root: PathBuf) -> Result<Self> {
        Ok(Self {
            config: Config::parse(DEFAULT_CONFIG)?,
            root,
            local: None,
            basis: Basis::default(),
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
        // The candidate was never read by the user, so it is never trusted.
        Self {
            local,
            basis: Basis::default(),
            ..self
        }
    }

    /// Plugins that run in this process only: the bundled ones and the local
    /// decisions. Nothing is started, so it is cheap and cannot fail on a
    /// language plugin.
    pub fn in_process_registry(&self) -> Result<Registry> {
        let mut registry = lighthouse_checks::registry();
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
/// current directory. Nothing of the project is loaded, so a broken decision or
/// catalog cannot get in the way.
pub fn project_root() -> Result<PathBuf> {
    let here = env::current_dir()?;
    let found = here.ancestors().find(|dir| Config::file_in(dir).is_some());
    Ok(found.map_or(here.clone(), Path::to_owned))
}

/// The catalog of the project at `root`: the bundled one with the project's
/// layer on top. Fails when the project's layer is broken.
pub fn catalog_at(root: &Path) -> Result<Catalog> {
    layered(load_local(root)?.as_ref())
}

/// Every command the project would run: the formatters of its configuration
/// and the command fixers and command checks of its catalog.
fn commands(config: &Config, catalog: &Catalog) -> Vec<Command> {
    let mut found: Vec<Command> = config
        .formatters()
        .map(|(language, f)| Command {
            what: format!("formatter for {language}: {}", f.argv.join(" ")),
            argv: f.argv.clone(),
        })
        .collect();
    for decision in catalog.decisions() {
        if let Some(check) = &decision.check
            && let CheckKind::Command(command) = &check.kind
        {
            found.push(Command {
                what: format!("check of {}: {}", decision.id(), command.argv.join(" ")),
                argv: command.argv.clone(),
            });
        }
        if let Some(fix) = &decision.fix
            && let FixKind::Command(command) = &fix.kind
        {
            found.push(Command {
                what: format!("fixer of {}: {}", decision.id(), command.argv.join(" ")),
                argv: command.argv.clone(),
            });
        }
    }
    found
}

fn layered(local: Option<&Catalog>) -> Result<Catalog> {
    match local {
        Some(local) => Ok(Catalog::overlay(Catalog::bundled(), local)?),
        None => Ok(Catalog::bundled().clone()),
    }
}
