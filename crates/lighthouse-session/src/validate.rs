//! `lighthouse spec validate`: every spec document under some paths is read
//! against its kind, and what the documents say about each other is checked:
//! a decision's check and fix against the rules the bundled
//! plugins register, a project's rules and the projects it extends against the
//! decisions and projects that exist, a decision's CEL against the CEL
//! compiler. Examples are checked for shape with the decision; running them
//! is `decision test` (or `--examples`).

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use lighthouse_checks::Composition;
use lighthouse_model::{Format, Resource, documents, kind_of, resource};
use lighthouse_plugin::{Registry, plugin_of};
use lighthouse_rpc::PluginSpec;
use lighthouse_spec::Catalog;
use lighthouse_spec::{Config, ProjectSpec};
use serde_json::Value;

use crate::{Result, SKIPPED_DIRS, Session, decisions::test_catalog};

/// The decision that asks decisions to be named well.
const DECISION_NAMING: &str = "core/decision-naming";

/// One thing wrong with one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub path: String,
    pub message: String,
}

/// What a validation read and found.
#[derive(Debug, Default)]
pub struct Validated {
    pub files: usize,
    pub documents: usize,
    pub problems: Vec<Problem>,
}

/// The languages the project's plugins provide, or why that is not known.
type Served = std::result::Result<BTreeSet<String>, String>;

/// The documents found, by what is done with them.
#[derive(Default)]
struct Sets {
    /// Catalog files (`Pack`, `Decision`, `SourceMap`) by
    /// the directory they lie in, with their path and text.
    catalogs: BTreeMap<PathBuf, BTreeMap<String, String>>,
    projects: Vec<(String, Resource<ProjectSpec>)>,
    /// Whether a directory holds a pack, so that it is a catalog root.
    packs: BTreeSet<PathBuf>,
}

impl Sets {
    fn add(&mut self, file: &Path, doc: Value, problems: &mut Vec<Problem>) {
        let label = file.display().to_string();
        let Some(kind) = kind_of(&doc).map(str::to_owned) else {
            problems.push(problem(&label, "no `kind`: not a resource"));
            return;
        };
        match kind.as_str() {
            "Project" => match resource::<ProjectSpec>(&label, &doc) {
                Ok(r) => self.projects.push((label, r)),
                Err(e) => problems.push(problem(&label, e)),
            },
            "Preset" | "DecisionOverride" => {
                problems.push(problem(&label, format!("`{kind}` is not a kind")))
            }
            "Plugin" => {
                if let Err(e) = resource::<PluginSpec>(&label, &doc) {
                    problems.push(problem(&label, e));
                }
            }
            "Pack" | "Decision" | "SourceMap" => {
                let dir = file.parent().unwrap_or(Path::new("")).to_owned();
                if kind == "Pack" {
                    self.packs.insert(dir.clone());
                }
                // Catalog loading re-reads the file; keep its text whole.
                if let Ok(text) = fs::read_to_string(file) {
                    self.catalogs
                        .entry(dir)
                        .or_default()
                        .insert(file.display().to_string(), text);
                }
            }
            other => problems.push(problem(&label, format!("unsupported kind `{other}`"))),
        }
    }

    fn check(
        &self,
        session: &Session,
        registry: &Registry,
        examples: bool,
        problems: &mut Vec<Problem>,
    ) -> Result<()> {
        let cel = session.comp.cel();
        let bundled: &Catalog = &session.base;
        let mut layered: Vec<(Catalog, Vec<String>)> = Vec::new();
        let known: BTreeSet<String> = session
            .catalog
            .projects()
            .names()
            .map(str::to_owned)
            .chain(self.projects.iter().map(|(_, p)| p.metadata.name.clone()))
            .collect();
        for (path, config) in &self.projects {
            self.project(path, config, &known, (session, registry), problems);
        }
        let mut roots = BTreeSet::new();
        let mut named = BTreeSet::new();
        let mut served: Option<Served> = None;
        for (dir, files) in &self.catalogs {
            let at = self.catalog_root(dir).unwrap_or_else(|| dir.clone());
            let (loaded, shipped) = match self.catalog_root(dir) {
                Some(root) if !roots.insert(root.clone()) => continue,
                Some(root) => (
                    Catalog::load(&root, cel).map(|c| (root.display().to_string(), c)),
                    None,
                ),
                None => (
                    Catalog::from_local(by_file_name(files), cel)
                        .and_then(|local| Catalog::overlay(bundled, &local))
                        .map(|c| (dir.display().to_string(), c)),
                    Some(bundled),
                ),
            };
            match loaded {
                Ok((place, catalog)) => {
                    uids(&catalog, &place, problems);
                    let served = served.get_or_insert_with(|| served_languages(session));
                    languages(&catalog, shipped, (served, &place), problems);
                    if named.insert(at.clone()) {
                        naming(session, &at, problems)?;
                    }
                    let own = catalog
                        .decisions()
                        .filter(|d| d.automated() && bundled.decision(d.id()).is_none())
                        .map(|d| d.id().to_owned())
                        .collect::<Vec<String>>();
                    built(&session.comp, &catalog, &own, &place, problems);
                    layered.push((catalog, own));
                }
                Err(e) => problems.push(problem(&dir.display().to_string(), e)),
            }
        }
        if examples && problems.is_empty() {
            for (catalog, own) in layered.iter().filter(|(_, own)| !own.is_empty()) {
                let report = test_catalog(session, catalog, own, None)?;
                problems.extend(report.failures.iter().map(|f| problem("examples", f)));
            }
        }
        Ok(())
    }

    /// The root of the catalog a directory belongs to: the parent of the
    /// directory of its pack. `None` when no pack lies above it: the files
    /// are a project's local layer.
    fn catalog_root(&self, dir: &Path) -> Option<PathBuf> {
        dir.ancestors()
            .find(|d| self.packs.contains(*d))
            .map(|pack| pack.parent().unwrap_or(pack).to_owned())
    }

    fn project(
        &self,
        path: &str,
        project: &Resource<ProjectSpec>,
        known: &BTreeSet<String>,
        (session, registry): (&Session, &Registry),
        problems: &mut Vec<Problem>,
    ) {
        let config = match Config::from_resource(project.clone()) {
            Ok(config) => config,
            Err(e) => {
                problems.push(problem(path, e));
                return;
            }
        };
        let bundled = match crate::libraries()
            .and_then(|libs| lighthouse_checks::ids(&session.comp, &libs).map_err(Into::into))
        {
            Ok(ids) => ids,
            Err(e) => {
                problems.push(problem(path, e));
                return;
            }
        };
        for listed in config.plugins().iter().filter(|p| bundled.contains(&p.id)) {
            problems.push(problem(
                path,
                format!(
                    "plugin `{}` is bundled: remove it from `plugins`",
                    listed.id
                ),
            ));
        }
        let provided = match provided_by(session, &config, path) {
            Ok(provided) => provided,
            Err(why) => {
                problems.push(problem(path, why));
                return;
            }
        };
        let missing = |name: &str| {
            let pack = name.split_once('/')?.0;
            let known = bundled.iter().any(|id| id == pack)
                || pack == crate::authoring::LOCAL_PACK
                || config.lists(pack)
                || provided.packs().iter().any(|p| p.id == pack);
            (!known).then(|| {
                let plugin = lighthouse_rpc::plugin_of_pack(
                    pack,
                    &[],
                    &lighthouse_rpc::search_dirs(&session.root),
                );
                format!("`{name}`: pack `{pack}` comes from plugin `{plugin}`; add it to `plugins`")
            })
        };
        let rules = project
            .spec
            .rules
            .keys()
            .chain(project.spec.overrides.iter().flat_map(|o| o.rules.keys()));
        for name in project.spec.extends.iter().chain(rules) {
            if let Some(why) = missing(name) {
                problems.push(problem(path, why));
            }
        }
        for name in project.spec.extends.iter().filter(|n| {
            !known.contains(*n) && !provided.projects().names().any(|p| p == n.as_str())
        }) {
            if missing(name).is_none() {
                problems.push(problem(path, format!("extends unknown project `{name}`")));
            }
        }
        let rules = project
            .spec
            .rules
            .keys()
            .chain(project.spec.overrides.iter().flat_map(|o| o.rules.keys()));
        known_rules(path, rules, registry, &provided, problems);
    }
}

/// Validates the documents under `paths` (files or directories).
/// `examples` also runs the examples of the decisions found, in the languages
/// the project's plugins provide.
pub fn validate_paths(session: &Session, paths: &[PathBuf], examples: bool) -> Result<Validated> {
    let mut found = Validated::default();
    let mut files = Vec::new();
    for path in paths {
        collect(path, &mut files)?;
    }
    files.sort();
    files.dedup();
    let registry = registry_of(session)?;
    let mut sets = Sets::default();
    for file in &files {
        let Some(format) = Format::of_path(file) else {
            continue;
        };
        found.files += 1;
        let label = file.display().to_string();
        let text = fs::read_to_string(file)?;
        match documents(format, &label, &text) {
            Ok(docs) => {
                for doc in docs {
                    found.documents += 1;
                    sets.add(file, doc, &mut found.problems);
                }
            }
            Err(e) => found.problems.push(problem(&label, e)),
        }
    }
    sets.check(session, &registry, examples, &mut found.problems)?;
    Ok(found)
}

/// The bundled catalog with the catalogs of the plugins `config` lists, found
/// from the directory of the document at `path`.
fn provided_by(
    session: &Session,
    config: &Config,
    path: &str,
) -> std::result::Result<Catalog, String> {
    let dir = Path::new(path).parent().unwrap_or(Path::new(""));
    let search = lighthouse_rpc::search_dirs(&session.root);
    let listed =
        lighthouse_rpc::plugin_catalogs(config, dir, &search).map_err(|e| e.to_string())?;
    crate::project::shipped(&session.comp, &listed).map_err(|e| e.to_string())
}

/// The rules that run in this process. A local decision that does not build
/// is reported with the documents that hold it, not as a failure of
/// validating them, so it is left out.
fn registry_of(session: &Session) -> Result<Registry> {
    session.in_process_registry().or_else(|_| {
        lighthouse_checks::registry(&session.comp, crate::libraries()?)
            .map_err(|e| e.to_string().into())
    })
}

/// What building the checks and fixes of the decisions `own` says is wrong
/// with them: an expression that does not compile, a function that does not
/// exist, a variable a function cannot see.
fn built(
    comp: &Composition,
    catalog: &Catalog,
    own: &[String],
    place: &str,
    problems: &mut Vec<Problem>,
) {
    let packs: BTreeSet<&str> = own
        .iter()
        .filter_map(|id| id.split_once('/').map(|(pack, _)| pack))
        .collect();
    for pack in packs {
        if let Err(e) = lighthouse_checks::Checks::new(comp, pack, catalog) {
            problems.push(problem(place, e));
        }
    }
}

fn collect(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if path.is_file() {
        files.push(path.to_owned());
        return Ok(());
    }
    if !path.is_dir() {
        return Err(format!("{}: no such file or directory", path.display()).into());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?.path();
        let name = entry
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if entry.is_dir() {
            if !SKIPPED_DIRS.contains(&name) {
                collect(&entry, files)?;
            }
        } else {
            files.push(entry);
        }
    }
    Ok(())
}

fn problem(path: &str, message: impl ToString) -> Problem {
    Problem {
        path: path.to_owned(),
        message: message.to_string(),
    }
}

/// The decisions written under `root` are named the way `core/decision-naming`
/// asks: the rule runs over them like it does over any project, and each
/// finding is a problem of the file it is in.
fn naming(session: &Session, root: &Path, problems: &mut Vec<Problem>) -> Result<()> {
    let config = Config::parse_inline(&format!(
        "plugins = [\"core\"]\n[rules]\n\"{DECISION_NAMING}\" = \"warn\"\n"
    ))?;
    let engine =
        session.engine_with(registry_of(session)?, config, session.comp.bundled()?, root)?;
    let outcome = engine.check(&[], &[DECISION_NAMING.to_owned()])?;
    problems.extend(outcome.diagnostics.iter().map(|d| {
        let at = root.join(&d.file);
        problem(
            &format!("{}:{}", at.display(), d.span.start.line),
            &d.message,
        )
    }));
    Ok(())
}

/// A rule named in configuration exists, when its plugin is one that runs in
/// this process; rules of external plugins cannot be known without starting
/// them.
fn known_rules<'a>(
    path: &str,
    rules: impl Iterator<Item = &'a String>,
    registry: &Registry,
    bundled: &Catalog,
    problems: &mut Vec<Problem>,
) {
    for id in rules {
        let in_process = registry.has_plugin(plugin_of(id));
        if in_process && registry.rule(id).is_none() && bundled.decision(id).is_none() {
            problems.push(problem(
                path,
                format!("rule `{id}` is not registered by its plugin"),
            ));
        }
    }
}

fn by_file_name(files: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    files
        .iter()
        .map(|(path, text)| {
            let name = Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            (name, text.clone())
        })
        .collect()
}

/// The ids of the languages the project's plugins provide, started for the
/// purpose, or why that is not known.
fn served_languages(session: &Session) -> Served {
    let (registry, registered) = session.registry().map_err(|e| e.to_string())?;
    if let Some(gap) = registered.incomplete.first() {
        return Err(format!("a language plugin did not start: {}", gap.reason));
    }
    Ok(registry
        .languages()
        .iter()
        .map(|language| language.provider.manifest().id.clone())
        .collect())
}

/// A decision lists only languages a plugin provides, as a rule lists only the
/// languages its linter supports. The decisions of `shipped` are not asked,
/// since a project need not run every language they cover.
fn languages(
    catalog: &Catalog,
    shipped: Option<&Catalog>,
    (served, place): (&Served, &str),
    problems: &mut Vec<Problem>,
) {
    let listed = catalog
        .decisions()
        .filter(|d| !d.languages.is_empty())
        .filter(|d| shipped.is_none_or(|s| s.decision(d.id()).is_none()))
        .collect::<Vec<_>>();
    if listed.is_empty() {
        return;
    }
    let served = match served {
        Ok(served) => served,
        Err(why) => {
            problems.push(problem(
                place,
                format!("`languages` cannot be checked: {why}"),
            ));
            return;
        }
    };
    for decision in listed {
        for language in decision.languages.keys().filter(|l| !served.contains(*l)) {
            problems.push(problem(
                place,
                format!(
                    "{}: `languages` lists `{language}`, which no plugin of the project provides",
                    decision.id()
                ),
            ));
        }
    }
}

/// Every decision has the `uid` its findings are identified by.
fn uids(catalog: &Catalog, path: &str, problems: &mut Vec<Problem>) {
    for decision in catalog.decisions() {
        if decision.uid().is_none() {
            problems.push(problem(
                path,
                format!(
                    "{}: has no `metadata.uid`; add a UUID v4 under `metadata`",
                    decision.id()
                ),
            ));
        }
    }
}
