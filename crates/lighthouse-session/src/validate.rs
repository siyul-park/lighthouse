//! `lighthouse spec validate`: every spec document under some paths is read
//! against its kind, and what the documents say about each other is checked:
//! a decision's check and fix against the rules and order keys the bundled
//! plugins register, a project's rules and the projects it extends against the
//! decisions and projects that exist, a decision's CEL against the CEL
//! compiler. Examples are checked for shape with the decision; running them
//! is `decision test` (or `--examples`).

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use lighthouse_plugin::{Registry, plugin_of};
use lighthouse_resource::{Format, Resource, documents, kind_of, resource};
use lighthouse_rpc::PluginSpec;
use lighthouse_spec::{BuiltinCheck, BuiltinOp, Catalog, CheckKind, Decision, FixKind, OpSpec};
use lighthouse_spec::{Config, ProjectSpec};
use serde_json::Value;

use crate::{Result, SKIPPED_DIRS, Session, decisions::test_catalog};

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
            problems.push(problem(
                &label,
                "no `kind`: not a resource (run `lighthouse spec migrate`)",
            ));
            return;
        };
        match kind.as_str() {
            "Project" => match resource::<ProjectSpec>(&label, &doc) {
                Ok(r) => self.projects.push((label, r)),
                Err(e) => problems.push(problem(&label, e)),
            },
            "Preset" | "DecisionOverride" => problems.push(problem(
                &label,
                format!("`{kind}` is not a kind any more (run `lighthouse spec migrate`)"),
            )),
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
        let bundled = Catalog::bundled();
        let mut layered: Vec<(Catalog, Vec<String>)> = Vec::new();
        let known: BTreeSet<String> = session
            .catalog()?
            .projects()?
            .names()
            .map(str::to_owned)
            .chain(self.projects.iter().map(|(_, p)| p.metadata.name.clone()))
            .collect();
        for (path, config) in &self.projects {
            self.project(path, config, &known, registry, bundled, problems);
        }
        let mut roots = BTreeSet::new();
        for (dir, files) in &self.catalogs {
            let loaded = match self.catalog_root(dir) {
                Some(root) if !roots.insert(root.clone()) => continue,
                Some(root) => Catalog::load(&root).map(|c| (root.display().to_string(), c)),
                None => Catalog::from_local(local_files(files))
                    .and_then(|local| Catalog::overlay(bundled, &local))
                    .map(|c| (dir.display().to_string(), c)),
            };
            match loaded {
                Ok((place, catalog)) => {
                    references(&catalog, registry, &place, problems);
                    let own = catalog
                        .decisions()
                        .filter(|d| d.automated() && bundled.decision(d.id()).is_none())
                        .map(|d| d.id().to_owned())
                        .collect();
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
        registry: &Registry,
        bundled: &Catalog,
        problems: &mut Vec<Problem>,
    ) {
        if let Err(e) = Config::from_resource(project.clone()) {
            problems.push(problem(path, e));
            return;
        }
        for name in project.spec.extends.iter().filter(|n| !known.contains(*n)) {
            problems.push(problem(path, format!("extends unknown project `{name}`")));
        }
        let rules = project
            .spec
            .rules
            .keys()
            .chain(project.spec.overrides.iter().flat_map(|o| o.rules.keys()));
        known_rules(path, rules, registry, bundled, problems);
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
    let registry = session.in_process_registry()?;
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

fn local_files(files: &BTreeMap<String, String>) -> BTreeMap<String, String> {
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

/// What a decision refers to outside itself exists.
fn references(catalog: &Catalog, registry: &Registry, path: &str, problems: &mut Vec<Problem>) {
    let keys: BTreeSet<&str> = registry
        .order_keys()
        .map(|k| k.manifest().id.as_str())
        .collect();
    for decision in catalog.decisions() {
        order_keys(decision, &keys, path, problems);
        if let Some(CheckKind::Builtin(builtin)) = decision.check.as_ref().map(|c| &c.kind) {
            named_rule(decision, builtin, registry, path, problems);
            if let BuiltinCheck::Op(BuiltinOp::Order { clauses }) = builtin {
                for clause in clauses {
                    unknown_keys(decision, &clause.by, &keys, path, problems);
                }
            }
        }
    }
}

fn named_rule(
    decision: &Decision,
    builtin: &BuiltinCheck,
    registry: &Registry,
    path: &str,
    problems: &mut Vec<Problem>,
) {
    let Some(id) = builtin.named() else {
        return;
    };
    if registry.has_plugin(plugin_of(id)) && registry.rule(id).is_none() {
        problems.push(problem(
            path,
            format!(
                "{}: builtin check `{id}` is not registered by its plugin",
                decision.id()
            ),
        ));
    }
}

fn unknown_keys(
    decision: &Decision,
    by: &[String],
    keys: &BTreeSet<&str>,
    path: &str,
    problems: &mut Vec<Problem>,
) {
    for key in by.iter().filter(|key| !keys.contains(key.as_str())) {
        problems.push(problem(
            path,
            format!("{}: `order` names unknown order key `{key}`", decision.id()),
        ));
    }
}

fn order_keys(decision: &Decision, keys: &BTreeSet<&str>, path: &str, problems: &mut Vec<Problem>) {
    let Some(fix) = &decision.fix else {
        return;
    };
    let FixKind::Ops { ops } = &fix.kind else {
        return;
    };
    for op in ops {
        let OpSpec::Reorder { by, .. } = op else {
            continue;
        };
        for key in by {
            if !keys.contains(key.as_str()) {
                problems.push(problem(
                    path,
                    format!("{}: reorder names unknown order key `{key}`", decision.id()),
                ));
            }
        }
    }
}
