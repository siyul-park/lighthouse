//! What every command that reads a project needs: its configuration, the
//! registry of bundled, project-local and language plugins, and the project's
//! own catalog layer.

use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use lighthouse_checks::Checks;
use lighthouse_engine::{CommandModels, Engine};
use lighthouse_plugin::{ModelClient, Registry};
use lighthouse_resource::Format;
use lighthouse_rpc::{CommandModel, Registered};
use lighthouse_spec::{Catalog, CheckKind, FixKind, load_local, local_files};
use lighthouse_spec::{Config, FILE_NAME, Layers};

use crate::{
    Result, environment,
    permit::permitted,
    trust::{Basis, Program},
};

/// What `init` writes and what a project without a config is checked with.
pub const DEFAULT_CONFIG: &str = "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Project\"\n\n[metadata]\nname = \"project\"\n\n[spec]\nplugins = [\"core\"]\nextends = [\"core/recommended\"]\n";

/// What every run says when the sandbox is off.
const SANDBOX_OFF: &str = "sandbox off: programs run with your full access";

/// A project as the frontends open it: its configuration, its root directory
/// and its decisions.
#[derive(Clone)]
pub struct Session {
    pub config: Config,
    pub root: PathBuf,
    /// The root of the store of datasets and trained models: `root`, or the
    /// first repository's when a run spans several, so that every repository
    /// finds the model trained on all of them.
    pub store: PathBuf,
    /// The catalog layer of `.lighthouse/decisions`, when the project has one.
    local: Option<Catalog>,
    /// The bundled catalog with the project's layer on top.
    pub catalog: Arc<Catalog>,
    /// What the project asks to be trusted for, computed from the same bytes
    /// the configuration and the decisions were loaded from.
    pub basis: Basis,
    /// The environment of the configuration, layer by layer.
    pub(crate) layers: Layers,
}

impl Session {
    /// Loads `config`, or discovers `lighthouse.toml` upward from the current
    /// directory, and the project's local decisions.
    pub fn load(config: Option<&Path>) -> Result<Self> {
        Self::load_in(&env::current_dir()?, config)
    }

    /// Like `load`, for the project `dir` belongs to.
    pub fn load_in(dir: &Path, config: Option<&Path>) -> Result<Self> {
        Self::open(dir, config, true)
    }

    /// Like `load`, but a missing `lighthouse.toml` means the default
    /// configuration over the current directory.
    pub fn load_or_default(config: Option<&Path>) -> Result<Self> {
        Self::open(&env::current_dir()?, config, false)
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

    /// The same project keeping its datasets and models in the store at
    /// `store`.
    pub fn stored_in(self, store: &Path) -> Self {
        Self {
            store: store.to_owned(),
            ..self
        }
    }

    fn open(here: &Path, config: Option<&Path>, required: bool) -> Result<Self> {
        if let Some(path) = config {
            let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            return Self::assemble(path, &text, here.to_owned());
        }
        match Self::find_in(here)? {
            Some(session) => Ok(session),
            None if required => Err(format!("no {FILE_NAME} found (run `lighthouse init`)").into()),
            None => Self::assemble(Path::new(FILE_NAME), DEFAULT_CONFIG, here.to_owned()),
        }
    }

    /// Builds the session from the text of the configuration file at `path`
    /// and the decision files read here, once: what is parsed is what trust is
    /// judged on.
    fn assemble(path: &Path, text: &str, root: PathBuf) -> Result<Self> {
        let format = Format::of_path(path).unwrap_or(Format::Toml);
        let personal = environment::read(&root)?;
        let covered: Vec<(&str, String)> = personal
            .iter()
            .filter_map(|p| Some((p.layer.origin.as_str(), p.trusted()?)))
            .collect();
        let texts: Vec<(&str, &str)> = covered
            .iter()
            .map(|(origin, trusted)| (*origin, trusted.as_str()))
            .collect();
        let layers = personal.iter().map(|p| p.layer.clone()).collect();
        let (config, layers) = Layers::build(format, &path.display().to_string(), text, layers)?;
        let files = local_files(&root)?;
        let local = files.clone().map(Catalog::from_local).transpose()?;
        let catalog = layered(local.as_ref())?;
        environment::check_local(&root, &personal, &catalog)?;
        let commands = commands(&config, &catalog, &root)?;
        let basis = Basis::of(&root, text, &texts, &files.unwrap_or_default(), &commands);
        Ok(Self {
            config,
            store: root.clone(),
            root,
            local,
            catalog: Arc::new(catalog),
            basis,
            layers,
        })
    }

    /// A session over `root` with the default configuration and no local
    /// decisions: for commands that must work on a project whose own files
    /// are broken or out of date.
    pub fn bare(root: PathBuf) -> Result<Self> {
        Ok(Self {
            config: Config::parse(DEFAULT_CONFIG)?,
            store: root.clone(),
            root,
            local: None,
            catalog: Arc::new(layered(None)?),
            basis: Basis::default(),
            layers: Layers::default(),
        })
    }

    /// The bundled and local plugins plus the language plugins the config
    /// lists, which are started here. A plugin that serves models is started
    /// in the store.
    pub fn registry(&self) -> Result<(Registry, Registered)> {
        let mut registry = self.in_process_registry()?;
        let search = lighthouse_rpc::search_dirs(&self.root);
        if !self.trusted()
            && let Some(local) =
                lighthouse_rpc::local_plugins(&self.config, &self.root, &search).first()
        {
            return Err(format!(
                "plugin `{}` comes with the project and would run a program: this project is not trusted (run `lighthouse trust`, or set LIGHTHOUSE_TRUST=1 in CI)",
                local.id
            )
            .into());
        }
        let registered = lighthouse_rpc::register(
            &mut registry,
            &self.config,
            &self.root,
            &self.store,
            &search,
        )?;
        Ok((registry, registered))
    }

    /// The engine of the project with its plugins started, and what starting
    /// them had to say. It trusts what the user trusts and runs the models the
    /// project's providers name.
    pub fn engine(&self) -> Result<(Engine, Vec<String>)> {
        let sandbox = self.config.sandbox().clone();
        if !sandbox.enabled && !self.trusted() {
            return Err(
                "`[sandbox] enabled = false` needs a trusted project: programs would run with your full access (run `lighthouse trust`)"
                    .into(),
            );
        }
        let (registry, plugins) = self.registry()?;
        let root = self.root.clone();
        let policy = sandbox.clone();
        let commands: CommandModels = Arc::new(move |name, model, provider| {
            CommandModel::new(&root, name, model, provider, &policy)
                .map(|command| Box::new(command) as Box<dyn ModelClient>)
        });
        let engine = Engine::new(registry, self.config.clone(), &self.catalog, &self.root)?
            .with_incomplete(plugins.incomplete)
            .with_trust(self.trusted())
            .with_commands(commands);
        let listed = permitted(&self.config, &self.catalog, &|id| engine.active(id))?;
        let mut notices = plugins.notices;
        if !sandbox.enabled {
            notices.push(SANDBOX_OFF.to_owned());
        }
        notices.extend(listed.warnings);
        Ok((engine, notices))
    }

    /// The same project with another local layer, such as a candidate that
    /// has not been written yet. Fails when the layer does not stack on the
    /// bundled one.
    pub fn with_local(&self, local: Option<Catalog>) -> Result<Self> {
        // The candidate was never read by the user, so it is never trusted.
        Ok(Self {
            catalog: Arc::new(layered(local.as_ref())?),
            local,
            basis: Basis::default(),
            ..self.clone()
        })
    }

    /// Plugins that run in this process only: the bundled ones and the local
    /// decisions. Nothing is started, so it is cheap and cannot fail on a
    /// language plugin.
    pub fn in_process_registry(&self) -> Result<Registry> {
        let mut registry = lighthouse_checks::registry();
        if let Some(local) = &self.local {
            let plugin = Checks::new("local", local)?;
            if !plugin.is_empty() {
                registry.register(&plugin)?;
            }
        }
        Ok(registry)
    }
}

/// The project root as commands that need no plugins see it: the current
/// directory when `config` names the project file, else the directory of the
/// `lighthouse.toml` found upward from it, else the current directory. Nothing
/// of the project is loaded, so a broken decision or catalog cannot get in
/// the way.
pub fn project_root(config: Option<&Path>) -> Result<PathBuf> {
    let here = env::current_dir()?;
    if config.is_some() {
        return Ok(here);
    }
    let found = here.ancestors().find(|dir| Config::file_in(dir).is_some());
    Ok(found.map_or(here.clone(), Path::to_owned))
}

/// The configuration file in `root`, if it has one.
pub fn config_file(root: &Path) -> Option<PathBuf> {
    Config::file_in(root)
}

/// The catalog of the project at `root`: the bundled one with the project's
/// layer on top. Fails when the project's layer is broken.
pub fn catalog_at(root: &Path) -> Result<Catalog> {
    layered(load_local(root)?.as_ref())
}

/// Every program the project would run: the programs of the fix
/// verification of its configuration, its command model providers, and the
/// command fixes and command checks of its catalog that are not the bundled
/// catalog's own, and the plugins that come with the project.
fn commands(config: &Config, catalog: &Catalog, root: &Path) -> Result<Vec<Program>> {
    let mut found: Vec<Program> = config
        .fix()
        .verify
        .iter()
        .map(|program| Program {
            what: format!("fix verification: {}", program.command.join(" ")),
            argv: program.command.clone(),
        })
        .collect();
    for local in lighthouse_rpc::local_plugins(config, root, &lighthouse_rpc::search_dirs(root)) {
        found.push(Program {
            what: format!("plugin {}: {}", local.id, local.argv.join(" ")),
            argv: local.argv,
        });
    }
    for (name, provider) in &config.models(catalog.projects())?.providers {
        if let lighthouse_spec::ModelProvider::Command { program, .. } = provider {
            found.push(Program {
                what: format!("model provider {name}: {}", program.command.join(" ")),
                argv: program.command.clone(),
            });
        }
    }
    for decision in catalog.decisions().filter(|d| !d.bundled()) {
        if let Some(check) = &decision.check
            && let CheckKind::Command(command) = &check.kind
        {
            let argv = &command.program.command;
            found.push(Program {
                what: format!("check of {}: {}", decision.id(), argv.join(" ")),
                argv: argv.clone(),
            });
        }
        if let Some(fix) = &decision.fix
            && let FixKind::Command(command) = &fix.kind
        {
            let argv = &command.program.command;
            found.push(Program {
                what: format!("fixer of {}: {}", decision.id(), argv.join(" ")),
                argv: argv.clone(),
            });
        }
    }
    Ok(found)
}

fn layered(local: Option<&Catalog>) -> Result<Catalog> {
    match local {
        Some(local) => Ok(Catalog::overlay(Catalog::bundled(), local)?),
        None => Ok(Catalog::bundled().clone()),
    }
}
