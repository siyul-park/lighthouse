//! What every command that reads a project needs: its configuration, the
//! registry of bundled, project-local and language plugins, and the project's
//! own catalog layer.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use lighthouse_checks::{Checks, Composition};
use lighthouse_engine::{CommandModels, Engine};
use lighthouse_plugin::{ModelClient, Registry};
use lighthouse_resource::Format;
use lighthouse_rpc::{CommandModel, PluginCatalog, Registered};
use lighthouse_spec::{Catalog, CheckKind, Files, FixKind, load_local, local_files};
use lighthouse_spec::{Config, FILE_NAME, Layers};

use crate::{
    Result,
    authoring::LOCAL_PACK,
    environment,
    permit::permitted,
    trust::{Basis, Program},
};

/// What `init` writes and what a project without a config is checked with.
pub const DEFAULT_CONFIG: &str = "apiVersion = \"lighthouse/v1alpha1\"\nkind = \"Project\"\n\n[metadata]\nname = \"project\"\n\n[spec]\nextends = [\"core/recommended\"]\n";

/// Why a project that turns the sandbox off is not run before it is trusted.
const NO_SANDBOX_UNTRUSTED: &str = "`[sandbox] enabled = false` needs a trusted project: programs would run with your full access (run `lighthouse trust`)";

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
    /// The catalog as it ships: the bundled packs and the packs of the plugins
    /// the project lists.
    pub(crate) base: Arc<Catalog>,
    /// The catalogs of listed plugins that come with the project and are not
    /// loaded because the project is not trusted.
    withheld: Vec<PluginCatalog>,
    /// The shipped catalog with the project's layer on top.
    pub catalog: Arc<Catalog>,
    /// What the project asks to be trusted for, computed from the same bytes
    /// the configuration and the decisions were loaded from.
    pub basis: Basis,
    /// The environment of the configuration, layer by layer.
    pub(crate) layers: Layers,
    /// The CEL environment every decision of the session is compiled in.
    pub comp: Composition,
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
    pub(crate) fn stored_in(self, store: &Path) -> Self {
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
        let comp = crate::compose()?;
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
        let bundled = lighthouse_checks::ids(&comp)?;
        if let Some(listed) = config.plugins().iter().find(|p| bundled.contains(&p.id)) {
            return Err(format!(
                "plugin `{}` is bundled: remove it from `plugins`",
                listed.id
            )
            .into());
        }
        if let Some(listed) = config.lists(LOCAL_PACK).then_some(LOCAL_PACK) {
            return Err(format!(
                "plugin `{listed}` is the project's own decisions: remove it from `plugins`"
            )
            .into());
        }
        let files = local_files(&root)?;
        let local = files
            .clone()
            .map(|files| Catalog::from_local(files, comp.cel()))
            .transpose()?;
        let search = lighthouse_rpc::search_dirs(&root);
        let listed = lighthouse_rpc::plugin_catalogs(&config, &root, &search)?;
        let commands = commands(&config, local.as_ref(), &root);
        let basis = Basis::of(&root, text, &texts, &files.unwrap_or_default(), &commands);
        let trusted = basis.trusted(&root);
        let (loaded, withheld): (Vec<_>, Vec<_>) =
            listed.into_iter().partition(|c| trusted || !c.local);
        let base = shipped(&comp, &loaded)?;
        let catalog = layered(&base, local.as_ref())?;
        if withheld.is_empty() {
            environment::check_local(&root, &personal, &catalog)?;
        }
        Ok(Self {
            config,
            store: root.clone(),
            root,
            local,
            base: Arc::new(base),
            withheld,
            catalog: Arc::new(catalog),
            basis,
            layers,
            comp,
        })
    }

    /// A session over `root` with the default configuration and no local
    /// decisions: for commands that must work on a project whose own files
    /// are broken or out of date.
    pub fn bare(root: PathBuf) -> Result<Self> {
        let comp = crate::compose()?;
        Ok(Self {
            config: Config::parse(DEFAULT_CONFIG)?,
            store: root.clone(),
            root,
            local: None,
            base: Arc::new(comp.bundled()?.clone()),
            withheld: Vec::new(),
            catalog: Arc::new(comp.bundled()?.clone()),
            basis: Basis::default(),
            layers: Layers::default(),
            comp,
        })
    }

    /// The bundled and local plugins, the packs of the listed plugins, and the
    /// language plugins the config lists, which are started here. A plugin that serves models is started
    /// in the store.
    pub fn registry(&self) -> Result<(Registry, Registered)> {
        let mut registry = self.in_process_registry()?;
        let search = lighthouse_rpc::search_dirs(&self.root);
        if let Some(why) = self.untrusted_plugin(&search) {
            return Err(why.into());
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
            return Err(NO_SANDBOX_UNTRUSTED.into());
        }
        let (registry, plugins) = self.registry()?;
        self.require_packs(true, |pack| registry.has_plugin(pack))?;
        let config = self.enabled_in(&registry);
        let root = self.root.clone();
        let policy = sandbox.clone();
        let cel = self.comp.cel().clone();
        let commands: CommandModels = Arc::new(move |name, model, provider| {
            CommandModel::new(&cel, &root, name, model, provider, &policy)
                .map(|command| Box::new(command) as Box<dyn ModelClient>)
        });
        let engine = Engine::new(registry, config, &self.catalog, &self.root)?
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

    /// The configuration with every plugin of `registry` enabled: those the
    /// project lists, and those that need no listing (the bundled ones, the
    /// project's own decisions, the packs of the plugins it lists).
    pub(crate) fn enabled_in(&self, registry: &Registry) -> Config {
        self.config
            .clone()
            .enabling(registry.manifests().iter().map(|m| m.id.as_str()))
    }

    /// Why this project cannot be checked until the user trusts it, when that
    /// is so: it turns the sandbox off, or lists a plugin that comes with it.
    /// An untrusted project that needs neither is checked like any other.
    pub fn untrusted(&self) -> Option<String> {
        if self.trusted() {
            return None;
        }
        if !self.config.sandbox().enabled {
            return Some(NO_SANDBOX_UNTRUSTED.to_owned());
        }
        self.untrusted_plugin(&lighthouse_rpc::search_dirs(&self.root))
    }

    /// The error of a listed plugin that comes with the project in a project
    /// that is not trusted.
    fn untrusted_plugin(&self, search: &[PathBuf]) -> Option<String> {
        if self.trusted() {
            return None;
        }
        let local = lighthouse_rpc::local_plugins(&self.config, &self.root, search);
        local.first().map(|local| {
            format!(
                "plugin `{}` comes with the project and would run a program: this project is not trusted (run `lighthouse trust`, or set LIGHTHOUSE_TRUST=1 in CI)",
                local.id
            )
        })
    }

    /// Fails when the configuration names a pack that `provided` does not
    /// know, in `extends` and, with `rules`, in `rules` too, and says which
    /// plugin provides it.
    pub(crate) fn require_packs(&self, rules: bool, provided: impl Fn(&str) -> bool) -> Result<()> {
        let configured = self.config.configured().map(|(id, _)| id);
        let named = self
            .config
            .extends()
            .iter()
            .map(String::as_str)
            .chain(configured.filter(|_| rules));
        for name in named {
            let Some((pack, _)) = name.split_once('/') else {
                continue;
            };
            if provided(pack) || self.catalog.projects().get(name).is_some() {
                continue;
            }
            return Err(self.missing_pack(name, pack).into());
        }
        Ok(())
    }

    /// Why `name` of the pack `pack` cannot be resolved: the plugin that
    /// provides the pack is not listed, or comes with the project and is not
    /// trusted.
    fn missing_pack(&self, name: &str, pack: &str) -> String {
        let plugin = lighthouse_rpc::plugin_of_pack(
            pack,
            &self.withheld,
            &lighthouse_rpc::search_dirs(&self.root),
        );
        if self.withheld.iter().any(|c| c.plugin == plugin) {
            format!(
                "`{name}`: pack `{pack}` comes from plugin `{plugin}`, which comes with the project and is not loaded because this project is not trusted (run `lighthouse trust`, or set LIGHTHOUSE_TRUST=1 in CI)"
            )
        } else {
            format!("`{name}`: pack `{pack}` comes from plugin `{plugin}`; add it to `plugins`")
        }
    }

    /// Fails when `extends` names a pack the shipped catalog lacks.
    pub(crate) fn require_extended_packs(&self) -> Result<()> {
        self.require_packs(false, |pack| {
            pack == LOCAL_PACK || self.base.packs().iter().any(|p| p.id == pack)
        })
    }

    /// The same project with another local layer, such as a candidate that
    /// has not been written yet. Fails when the layer does not stack on the
    /// bundled one.
    pub(crate) fn with_local(&self, local: Option<Catalog>) -> Result<Self> {
        // The candidate was never read by the user, so it is never trusted.
        Ok(Self {
            catalog: Arc::new(layered(&self.base, local.as_ref())?),
            local,
            basis: Basis::default(),
            ..self.clone()
        })
    }

    /// Plugins that run in this process only: the bundled ones and the local
    /// decisions. Nothing is started, so it is cheap and cannot fail on a
    /// language plugin.
    pub fn in_process_registry(&self) -> Result<Registry> {
        let mut registry =
            lighthouse_checks::registry_of(&self.comp, &self.base).map_err(|e| e.to_string())?;
        if let Some(local) = &self.local {
            let plugin = Checks::new(&self.comp, "local", local)?;
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

/// The catalog of the project at `root`: the bundled one with the packs of the
/// plugins its configuration lists, and the project's layer on top. When the
/// configuration does not load, the bundled one with the project's layer. Fails
/// when the project's layer is broken.
pub fn catalog_at(root: &Path) -> Result<Catalog> {
    let comp = crate::compose()?;
    let local = load_local(root, comp.cel())?;
    if let Ok(Some(session)) = Session::find_in(root)
        && session.root == root
    {
        return Ok((*session.catalog).clone());
    }
    layered(comp.bundled()?, local.as_ref())
}

/// The bundled catalog with the catalogs of `plugins` added, as the plugins
/// ship them.
pub(crate) fn shipped(comp: &Composition, plugins: &[PluginCatalog]) -> Result<Catalog> {
    if plugins.is_empty() {
        return Ok(comp.bundled()?.clone());
    }
    let mut files = Files::new();
    for plugin in plugins {
        for (path, text) in Catalog::read_files(&plugin.dir)? {
            if files.insert(path.clone(), text).is_some() {
                return Err(format!(
                    "plugin `{}`: catalog file `{path}` is also in the catalog of another plugin",
                    plugin.plugin
                )
                .into());
            }
        }
    }
    Ok(Catalog::embedded_with(comp.cel(), files)?)
}

/// The bundled catalog with the catalogs of the plugins found in
/// `<root>/plugins` that the binary does not embed, and where the catalog of
/// each of their packs lies in the repository: what the docs of a repository
/// that develops plugins are generated from.
pub(crate) fn repository_catalog(
    comp: &Composition,
    root: &Path,
) -> Result<(Catalog, BTreeMap<String, String>)> {
    let bundled = lighthouse_checks::ids(comp)?;
    let found = lighthouse_rpc::discover(&[root.join("plugins")])?.found;
    let mut plugins = Vec::new();
    let mut homes = BTreeMap::new();
    for one in found
        .iter()
        .filter(|one| !bundled.iter().any(|id| id == one.id()))
    {
        let Some(dir) = one.catalog_dir() else {
            continue;
        };
        let home = format!("plugins/{}/decisions", one.id());
        for pack in Catalog::read_files(&dir)?
            .keys()
            .filter_map(|p| p.split_once('/'))
        {
            homes.insert(pack.0.to_owned(), home.clone());
        }
        plugins.push(PluginCatalog {
            plugin: one.id().to_owned(),
            dir,
            local: true,
        });
    }
    Ok((shipped(comp, &plugins)?, homes))
}

/// Every program the project would run: the programs of the fix
/// verification of its configuration, its command model providers, the
/// plugins that come with the project, and the command fixes and command
/// checks of its own decisions. The programs of a plugin's catalog are not
/// among them: that catalog is trusted with its plugin.
fn commands(config: &Config, local: Option<&Catalog>, root: &Path) -> Vec<Program> {
    let mut found: Vec<Program> = config
        .fix()
        .verify
        .iter()
        .map(|program| Program {
            what: format!("fix verification: {}", program.command.join(" ")),
            argv: program.command.clone(),
        })
        .collect();
    for plugin in lighthouse_rpc::local_plugins(config, root, &lighthouse_rpc::search_dirs(root)) {
        found.push(Program {
            what: format!("plugin {}: {}", plugin.id, plugin.argv.join(" ")),
            argv: plugin.argv,
        });
    }
    for (name, provider) in config.providers() {
        if let lighthouse_spec::ModelProvider::Command { program, .. } = provider {
            found.push(Program {
                what: format!("model provider {name}: {}", program.command.join(" ")),
                argv: program.command.clone(),
            });
        }
    }
    for decision in local.iter().flat_map(|local| local.decisions()) {
        let checks = decision.check.as_ref().and_then(|check| match &check.kind {
            CheckKind::Command(command) => Some(command.programs(decision)),
            _ => None,
        });
        let fixes = decision.fix.as_ref().and_then(|fix| match &fix.kind {
            FixKind::Command(command) => Some(command.programs(decision)),
            _ => None,
        });
        for (what, programs) in [("check", checks), ("fixer", fixes)] {
            for program in programs.iter().flat_map(|p| p.all()) {
                found.push(Program {
                    what: format!("{what} of {}: {}", decision.id(), program.shown()),
                    argv: program.command.clone(),
                });
            }
        }
    }
    found
}

fn layered(base: &Catalog, local: Option<&Catalog>) -> Result<Catalog> {
    match local {
        Some(local) => Ok(Catalog::overlay(base, local)?),
        None => Ok(base.clone()),
    }
}
