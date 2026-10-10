//! `lighthouse spec migrate`: rewrites the formats from before the resource
//! model as `lighthouse/v1alpha1` documents. Every document that already has
//! an `apiVersion` is left alone, so running it twice changes nothing.
//!
//! What it knows: `lighthouse.toml` (and its YAML or JSON equivalents),
//! `lighthouse-plugin.toml`, a catalog directory (`pack.yaml` with its
//! `section.yaml` files, pattern files, declarative rule files, `sources.yaml`)
//! and a project's `.lighthouse/rules`, which becomes `.lighthouse/decisions`.
//! A `Preset` becomes a `Project`, and a `DecisionOverride` of the project
//! becomes an entry of its `rules`.
//!
//! Only shapes it recognizes are touched: a pattern has an `id` and a
//! `requirement`, an override an `extends` and an override key, a
//! configuration one of its keys. Directory scans cover catalog directories
//! (those holding a `pack.yaml`), `.lighthouse/` and the configuration files;
//! other YAML or JSON is left alone. Changes are applied together: every
//! target is staged next to its destination and renamed into place before any
//! file is removed.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use lighthouse_resource::{Format, SCHEMA_URL_BASE, header, to_yaml, yaml_values};
use serde_json::Value as Json;
use serde_norway::{Mapping, Value};

pub mod catalog;
mod fold;
pub mod plugin;
pub mod project;
pub mod retired;

use crate::{Result, SKIPPED_DIRS};

/// Directories never searched for documents.
const LOCAL_ROOT: &str = ".lighthouse";
const LOCAL_RULES: &str = "rules";
const LOCAL_DECISIONS: &str = "decisions";

/// What a migration wrote, removed and left alone.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Migrated {
    pub written: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    /// Documents that already were resources.
    pub unchanged: usize,
    /// Files that were left as they are although they look like part of the
    /// old formats, with why.
    pub warnings: Vec<String>,
}

/// What to do with one file.
enum Action {
    Write(String),
    Remove,
}

enum FileKind {
    Project,
    Plugin,
    Yaml,
    Other,
}

/// A legacy YAML file of a catalog or of `.lighthouse/rules`, parsed.
struct Legacy {
    path: PathBuf,
    doc: Value,
}

/// The shapes of legacy documents that are recognized; anything else is not
/// Lighthouse's and stays as it is.
#[derive(PartialEq, Eq)]
enum Shape {
    /// A declarative rule file, consumed by the decisions that name it.
    RuleFile,
    /// A `section.yaml`, consumed by its `pack.yaml`.
    Section,
    Pack,
    Sources,
    Override,
    /// A `Preset` document: a `Project` now.
    Preset,
    /// A `DecisionOverride` document: an entry of the project's `rules` now.
    OverrideDoc,
    Pattern,
    /// A `Decision` that still has `enforcement` instead of `severity`.
    Enforcement,
    Foreign,
}

/// What the migration decided so far.
#[derive(Default)]
struct Plan {
    actions: BTreeMap<PathBuf, Action>,
    /// Rule files and `section.yaml` files, removed once a migrated document
    /// consumed them.
    consumable: BTreeSet<PathBuf>,
    consumed: BTreeSet<PathBuf>,
    /// Overrides to fold into the project's rules, by the override file.
    folds: Vec<(PathBuf, retired::Fold)>,
    /// Files kept although they look like part of the old formats.
    kept: Vec<String>,
}

/// What a pack needs from the files around its `pack.yaml`.
#[derive(Default)]
struct Surroundings {
    /// `section.yaml` files by pack directory and section id.
    sections: BTreeMap<PathBuf, BTreeMap<String, Legacy>>,
    /// Names of the override files by pack directory.
    overrides: BTreeMap<PathBuf, BTreeSet<String>>,
    /// How many documents each file holds, resources included.
    documents: BTreeMap<PathBuf, usize>,
}

impl Surroundings {
    fn of(legacy: &[Legacy], documents: BTreeMap<PathBuf, usize>) -> Self {
        let mut found = Self {
            documents,
            ..Self::default()
        };
        for item in legacy {
            let pack_dir = parent(&parent(&item.path));
            match shape(&item.path, &item.doc) {
                Shape::Section => {
                    let id = string(&item.doc, "id").unwrap_or_default();
                    found.sections.entry(pack_dir).or_default().insert(
                        id,
                        Legacy {
                            path: item.path.clone(),
                            doc: item.doc.clone(),
                        },
                    );
                }
                Shape::Override if !is_local(&item.path) => {
                    found
                        .overrides
                        .entry(pack_dir)
                        .or_default()
                        .insert(stem(&item.path));
                }
                _ => {}
            }
        }
        found
    }
}

impl Plan {
    /// Removes the rule and section files a migrated document consumed;
    /// returns a warning for each one nothing consumed, which stays.
    fn settle(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        for path in &self.consumable {
            if self.consumed.contains(path) {
                self.actions.insert(path.clone(), Action::Remove);
            } else {
                warnings.push(format!(
                    "{}: kept, no migrated document uses it",
                    path.display()
                ));
            }
        }
        warnings
    }
}

/// Migrates every legacy document under `paths` (files or directories). With
/// `dry_run` nothing is written or removed; the result says what would be.
pub fn migrate_paths(paths: &[PathBuf], dry_run: bool) -> Result<Migrated> {
    let files = scan(paths)?;
    let mut plan = Plan::default();
    let mut migrated = Migrated::default();
    let mut catalog_files = Vec::new();
    for file in &files {
        match kind_of_file(file) {
            FileKind::Project => {
                migrated.unchanged += project(file, &mut plan)?;
            }
            FileKind::Plugin => migrated.unchanged += plugin(file, &mut plan)?,
            FileKind::Yaml => catalog_files.push(file.clone()),
            FileKind::Other => {}
        }
    }
    migrated.unchanged += catalog(&catalog_files, &mut plan)?;
    fold::apply(&mut plan)?;
    migrated.warnings = plan.settle();
    migrated.warnings.append(&mut plan.kept);
    for (path, action) in &plan.actions {
        match action {
            Action::Write(_) => migrated.written.push(path.clone()),
            Action::Remove => migrated.removed.push(path.clone()),
        }
    }
    if !dry_run {
        apply(&plan.actions)?;
    }
    Ok(migrated)
}

/// Writes every target to a temporary file in its own directory, renames them
/// all into place, and only then removes files: a failure before the last
/// rename loses nothing.
fn apply(actions: &BTreeMap<PathBuf, Action>) -> Result<()> {
    let mut staged: Vec<(PathBuf, &Path)> = Vec::new();
    let cleanup = |staged: &[(PathBuf, &Path)]| {
        for (temp, _) in staged {
            let _ = fs::remove_file(temp);
        }
    };
    for (path, action) in actions {
        let Action::Write(text) = action else {
            continue;
        };
        let stage = || -> std::io::Result<PathBuf> {
            let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
            if let Some(dir) = dir {
                fs::create_dir_all(dir)?;
            }
            let temp = path.with_file_name(format!(
                ".{}.migrating-{}",
                file_name(path),
                std::process::id()
            ));
            fs::write(&temp, text)?;
            Ok(temp)
        };
        match stage() {
            Ok(temp) => staged.push((temp, path)),
            Err(e) => {
                cleanup(&staged);
                return Err(format!("{}: {e}", path.display()).into());
            }
        }
    }
    for (index, (temp, path)) in staged.iter().enumerate() {
        if let Err(e) = fs::rename(temp, path) {
            cleanup(&staged[index..]);
            return Err(format!("{}: {e}", path.display()).into());
        }
    }
    for (path, action) in actions {
        if matches!(action, Action::Remove) {
            fs::remove_file(path)?;
            remove_empty_parents(path);
        }
    }
    Ok(())
}

/// The files to look at under `paths`. A file given by name is always
/// included; a directory yields the configuration files, everything under a
/// `.lighthouse` directory, and the files of catalogs (directories holding a
/// `pack.yaml`, and the `sources.yaml` beside them).
fn scan(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut named = Vec::new();
    let mut found = Vec::new();
    for path in paths {
        collect(path, &mut named, &mut found)?;
    }
    let pack_dirs: BTreeSet<PathBuf> = found
        .iter()
        .chain(&named)
        .filter(|f| file_name(f) == "pack.yaml")
        .map(|f| parent(f))
        .collect();
    let in_scope = |file: &PathBuf| {
        is_config_name(file)
            || file
                .components()
                .any(|c| c.as_os_str().to_str() == Some(LOCAL_ROOT))
            || pack_dirs.iter().any(|dir| file.starts_with(dir))
            || (file_name(file) == "sources.yaml"
                && pack_dirs.iter().any(|dir| dir.parent() == file.parent()))
    };
    let mut files: Vec<PathBuf> = found.into_iter().filter(in_scope).chain(named).collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// Collects the files of `path` into `named` when it is a file and into
/// `found` when it was reached through a directory.
fn collect(path: &Path, named: &mut Vec<PathBuf>, found: &mut Vec<PathBuf>) -> Result<()> {
    if path.is_file() {
        named.push(path.to_owned());
        return Ok(());
    }
    if !path.is_dir() {
        return Err(format!("{}: no such file or directory", path.display()).into());
    }
    walk(path, found)
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?.path();
        if entry.is_dir() {
            if !SKIPPED_DIRS.contains(&file_name(&entry).as_str()) {
                walk(&entry, files)?;
            }
        } else {
            files.push(entry);
        }
    }
    Ok(())
}

fn is_config_name(path: &Path) -> bool {
    matches!(
        stem_of_name(path).as_str(),
        "lighthouse" | "lighthouse-plugin"
    ) && Format::of_path(path).is_some()
}

fn stem_of_name(path: &Path) -> String {
    file_name(path)
        .split('.')
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn kind_of_file(path: &Path) -> FileKind {
    match (stem_of_name(path).as_str(), Format::of_path(path)) {
        ("lighthouse", Some(_)) => FileKind::Project,
        ("lighthouse-plugin", Some(_)) => FileKind::Plugin,
        (_, Some(Format::Yaml)) => FileKind::Yaml,
        _ => FileKind::Other,
    }
}

fn project(path: &Path, plan: &mut Plan) -> Result<usize> {
    let name = path
        .canonicalize()
        .ok()
        .and_then(|p| {
            p.parent()?
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "project".to_owned());
    legacy_config(path, plan, project::is_legacy, |old| {
        project::migrate(old, &name)
    })
}

fn plugin(path: &Path, plan: &mut Plan) -> Result<usize> {
    legacy_config(path, plan, plugin::is_legacy, plugin::migrate)
}

/// A TOML, YAML or JSON document that is one resource: rewritten in its own
/// format when it has the legacy shape. A file of another shape is not ours
/// and stays; a file of several documents with a legacy one is refused.
fn legacy_config(
    path: &Path,
    plan: &mut Plan,
    recognize: impl Fn(&Json) -> bool,
    convert: impl Fn(&Json) -> std::result::Result<Json, String>,
) -> Result<usize> {
    let format = Format::of_path(path).unwrap_or(Format::Toml);
    let label = path.display().to_string();
    let text = fs::read_to_string(path)?;
    let docs = lighthouse_resource::documents(format, &label, &text)?;
    let unchanged = docs
        .iter()
        .filter(|d| d.get("apiVersion").is_some())
        .count();
    let legacy: Vec<&Json> = docs
        .iter()
        .filter(|d| d.get("apiVersion").is_none() && recognize(d))
        .collect();
    let Some(old) = legacy.first() else {
        return Ok(unchanged);
    };
    if docs.len() != 1 {
        return Err(several_documents(&label, docs.len()).into());
    }
    let new = convert(old).map_err(|e| format!("{label}: {e}"))?;
    let text = render_config(format, path, &new)?;
    plan.actions.insert(path.to_owned(), Action::Write(text));
    Ok(0)
}

/// `doc`, one `Project` or `Plugin` document, in the format of the file at `path`.
fn render_config(format: Format, path: &Path, doc: &Json) -> Result<String> {
    Ok(match format {
        Format::Toml => toml::to_string(doc)?,
        Format::Json => format!("{}\n", serde_json::to_string_pretty(doc)?),
        Format::Yaml => {
            let kind = doc["kind"].as_str().unwrap_or_default();
            format!("{}{}", header(kind, &schema_base(path)), to_yaml(doc))
        }
    })
}

fn several_documents(label: &str, count: usize) -> String {
    format!(
        "{label}: holds {count} documents and one of them is in a format from before the resource model; split it into one file per document and migrate again"
    )
}

/// Migrates the catalog files among `files`; returns how many documents were
/// already resources.
fn catalog(files: &[PathBuf], plan: &mut Plan) -> Result<usize> {
    let (legacy, documents, unchanged) = read_legacy(files)?;
    let around = Surroundings::of(&legacy, documents);
    let rule_files: Vec<&Legacy> = legacy
        .iter()
        .filter(|l| shape(&l.path, &l.doc) == Shape::RuleFile)
        .collect();
    for item in &legacy {
        plan_item(item, &around, &rule_files, plan)
            .map_err(|e| format!("{}: {e}", item.path.display()))?;
    }
    Ok(unchanged)
}

/// The legacy documents among the catalog files, how many documents each file
/// holds, and how many documents were already resources.
fn read_legacy(files: &[PathBuf]) -> Result<(Vec<Legacy>, BTreeMap<PathBuf, usize>, usize)> {
    let mut unchanged = 0;
    let mut legacy = Vec::new();
    let mut documents = BTreeMap::new();
    for path in files {
        let text = fs::read_to_string(path)?;
        let label = path.display().to_string();
        let docs = yaml_values(&label, &text)?;
        documents.insert(path.clone(), docs.len());
        for doc in docs {
            if catalog::is_resource(&doc)
                && !catalog::has_enforcement(&doc)
                && !retired::is_retired(&doc)
            {
                unchanged += 1;
            } else {
                legacy.push(Legacy {
                    path: path.clone(),
                    doc,
                });
            }
        }
    }
    Ok((legacy, documents, unchanged))
}

/// Which legacy shape a document has.
fn shape(path: &Path, doc: &Value) -> Shape {
    let name = file_name(path);
    if doc.get("kind").and_then(Value::as_str) == Some(retired::PRESET) {
        Shape::Preset
    } else if doc.get("kind").and_then(Value::as_str) == Some(retired::OVERRIDE) {
        Shape::OverrideDoc
    } else if catalog::has_enforcement(doc) {
        Shape::Enforcement
    } else if doc.get("select").is_some() && doc.get("where").is_some() && doc.get("id").is_none() {
        Shape::RuleFile
    } else if name == "section.yaml" && doc.is_mapping() && doc.get("id").is_some() {
        Shape::Section
    } else if name == "pack.yaml" && doc.get("sections").is_some() {
        Shape::Pack
    } else if name == "sources.yaml" && doc.is_sequence() {
        Shape::Sources
    } else if catalog::is_override(doc) {
        Shape::Override
    } else if doc.get("id").is_some() && doc.get("requirement").is_some() {
        Shape::Pattern
    } else {
        Shape::Foreign
    }
}

/// What to do with one legacy document.
fn plan_item(
    item: &Legacy,
    around: &Surroundings,
    rule_files: &[&Legacy],
    plan: &mut Plan,
) -> std::result::Result<(), String> {
    let (path, doc) = (&item.path, &item.doc);
    let shape = shape(path, doc);
    if shape == Shape::Foreign {
        return Ok(());
    }
    let count = around.documents.get(path).copied().unwrap_or(1);
    if count > 1 {
        return Err(several_documents("this file", count));
    }
    let target = is_local(path).then(|| local_target(path));
    let dir = parent(path);
    match shape {
        Shape::RuleFile | Shape::Section => {
            plan.consumable.insert(path.clone());
        }
        Shape::Pack => {
            let sections = section_docs(around.sections.get(&dir));
            let overrides = around.overrides.get(&dir).cloned().unwrap_or_default();
            let migrated = catalog::migrate_pack(doc, &sections, &overrides)?;
            for name in listed_sections(doc) {
                if let Some(section) = around.sections.get(&dir).and_then(|s| s.get(&name)) {
                    plan.consumed.insert(section.path.clone());
                }
            }
            write(plan, path, &migrated);
        }
        Shape::Sources => write(plan, path, &catalog::migrate_sources(doc)?),
        Shape::Override => {
            let migrated = catalog::migrate_override(doc, &stem(path))?;
            fold::plan(plan, path, &migrated)?;
        }
        Shape::OverrideDoc => fold::plan(plan, path, doc)?,
        Shape::Preset => {
            let project =
                retired::preset_to_project(serde_json::to_value(doc).map_err(|e| e.to_string())?);
            let project: Value = serde_norway::to_value(&project).map_err(|e| e.to_string())?;
            write(plan, path, &project);
        }
        Shape::Pattern => {
            let (migrated, rule) = decision(item, rule_files)?;
            plan.consumed.extend(rule);
            move_or_write(plan, path, target, &migrated)?;
        }
        Shape::Enforcement => {
            let mut migrated = doc.clone();
            catalog::convert_enforcement(&mut migrated)?;
            write(plan, path, &migrated);
        }
        Shape::Foreign => {}
    }
    Ok(())
}

/// The sections a `pack.yaml` lists, by name.
fn listed_sections(pack: &Value) -> Vec<String> {
    match pack.get("sections") {
        Some(Value::Sequence(names)) => names
            .iter()
            .filter_map(|n| n.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// The documents of the sections found beside a pack, by section id.
fn section_docs(sections: Option<&BTreeMap<String, Legacy>>) -> BTreeMap<String, Value> {
    sections
        .into_iter()
        .flatten()
        .map(|(id, l)| (id.clone(), l.doc.clone()))
        .collect()
}

fn parent(path: &Path) -> PathBuf {
    path.parent().unwrap_or(Path::new("")).to_owned()
}

/// The `Decision` of a pattern file, with its declarative rule inlined, and
/// the rule file it consumed.
fn decision(
    item: &Legacy,
    rule_files: &[&Legacy],
) -> std::result::Result<(Value, Option<PathBuf>), String> {
    let path = &item.path;
    let mut doc = item.doc.clone();
    let map = doc.as_mapping_mut().ok_or("a pattern is a mapping")?;
    let section = if is_local(path) {
        "rules".to_owned()
    } else {
        file_name(path.parent().unwrap_or(Path::new("")))
    };
    // A local rule file carries its rule inline; the loader named it.
    let inline = map.remove("rule");
    let name = file_name(path);
    if let Some(rule) = inline {
        let mut implementation = Mapping::new();
        implementation.insert("declarative".into(), Value::String(name.clone()));
        map.insert("implementation".into(), Value::Mapping(implementation));
        let migrated = catalog::migrate_decision(&doc, &section, Some((&name, &rule)))?;
        return Ok((migrated, None));
    }
    let declarative = map
        .get("implementation")
        .and_then(|i| i.get("declarative"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let Some(declared) = declarative else {
        return Ok((catalog::migrate_decision(&doc, &section, None)?, None));
    };
    let rule = find_rule(path, &declared, rule_files).ok_or_else(|| {
        format!("the declarative rule file `{declared}` was not found among the files given")
    })?;
    let migrated = catalog::migrate_decision(&doc, &section, Some((&declared, &rule.doc)))?;
    Ok((migrated, Some(rule.path.clone())))
}

/// The rule file a pattern at `pattern` names as `declared`. The old loader
/// read the name from the catalog root, the directory above the packs; a
/// name relative to the pattern's own directory is accepted too. Paths are
/// compared normalized, never by suffix.
fn find_rule<'a>(pattern: &Path, declared: &str, rule_files: &[&'a Legacy]) -> Option<&'a Legacy> {
    let catalog_root = parent(&parent(&parent(pattern)));
    [catalog_root, parent(pattern)]
        .iter()
        .map(|base| normal(&base.join(declared)))
        .find_map(|wanted| rule_files.iter().find(|r| normal(&r.path) == wanted))
        .copied()
}

/// `path` with `.` and `..` resolved, through the file system when the file
/// exists and lexically otherwise.
fn normal(path: &Path) -> PathBuf {
    if let Ok(real) = path.canonicalize() {
        return real;
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn is_local(path: &Path) -> bool {
    local_position(path).is_some()
}

/// Where a local rule goes: the same name in `.lighthouse/decisions`.
fn local_target(path: &Path) -> PathBuf {
    let at = local_position(path);
    path.components()
        .enumerate()
        .map(|(index, c)| {
            if Some(index) == at {
                Component::Normal(LOCAL_DECISIONS.as_ref())
            } else {
                c
            }
        })
        .collect()
}

/// Whether `path` lies in `.lighthouse/rules`: the position of `rules`.
fn local_position(path: &Path) -> Option<usize> {
    let names: Vec<_> = path.components().map(|c| c.as_os_str()).collect();
    names
        .windows(2)
        .position(|w| w[0] == LOCAL_ROOT && w[1] == LOCAL_RULES)
        .map(|at| at + 1)
}

fn move_or_write(
    plan: &mut Plan,
    path: &Path,
    target: Option<PathBuf>,
    migrated: &Value,
) -> std::result::Result<(), String> {
    match target {
        Some(target) => {
            if target.exists() {
                return Err(format!("{} already exists", target.display()));
            }
            plan.actions.insert(path.to_owned(), Action::Remove);
            write(plan, &target, migrated);
        }
        None => write(plan, path, migrated),
    }
    Ok(())
}

fn write(plan: &mut Plan, path: &Path, doc: &Value) {
    let kind = doc.get("kind").and_then(Value::as_str).unwrap_or_default();
    let text = format!("{}{}", header(kind, &schema_base(path)), to_yaml(doc));
    plan.actions.insert(path.to_owned(), Action::Write(text));
}

/// The directory of the schemas as a file at `path` reaches them: relative
/// when a `schema` directory with the schemas lies above it, else the URL
/// they are published at.
fn schema_base(path: &Path) -> String {
    let absolute = path
        .canonicalize()
        .or_else(|_| std::env::current_dir().map(|d| d.join(path)))
        .unwrap_or_else(|_| path.to_owned());
    let mut up = 0;
    let mut dir = absolute.parent();
    while let Some(here) = dir {
        if here.join("schema/decision.schema.json").is_file() {
            return format!("{}schema", "../".repeat(up));
        }
        up += 1;
        dir = here.parent();
    }
    SCHEMA_URL_BASE.to_owned()
}

fn string(doc: &Value, key: &str) -> Option<String> {
    doc.get(key)?.as_str().map(str::to_owned)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Removes the directories a removed file leaves empty.
fn remove_empty_parents(path: &Path) {
    let mut dir = path.parent();
    while let Some(here) = dir {
        if fs::remove_dir(here).is_err() {
            break;
        }
        dir = here.parent();
    }
}
